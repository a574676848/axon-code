use std::path::{Path, PathBuf};

use super::{
    AxonCli, CliRunResult, OutputFormat, json_result, run_session_store::resolved_axon_root,
};
use crate::management_commands::resolved_config_path;
use crate::runtime_workspace_instructions::load_workspace_instructions;
use crate::{StartupUpdateGate, run_startup_update_gate};
use axon_config::ConfigLoader;
use axon_tui_runtime::{
    AgentTuiLaunchRequest, AgentTuiRunOverrides, cleanup_agent_tui_tmp_dir,
    ensure_agent_tui_tmp_cleanup_started, launch_agent_tui_report,
};

mod catalog;
mod config_status;
mod question_answer_submission;
mod run_output_assistant_renderer;
mod run_output_generation_renderer;
mod run_output_memory_renderer;
mod run_output_patch_renderer;
mod run_output_plan_renderer;
#[cfg(test)]
mod run_output_projection_assistant_tests;
#[cfg(test)]
mod run_output_projection_diff_tests;
#[cfg(test)]
mod run_output_projection_failure_tests;
#[cfg(test)]
mod run_output_projection_memory_tests;
#[cfg(test)]
mod run_output_projection_reasoning_tests;
#[cfg(test)]
mod run_output_projection_review_tests;
#[cfg(test)]
mod run_output_projection_tests;
mod run_output_renderer;
mod run_output_review_renderer;
mod run_output_tool_renderer;
mod run_output_wire_sequence;
mod runtime_adapter;
mod settings_persistence;
mod tui_control_commands;
mod tui_submit_mapper;

const TUI_MAILBOX_TELEMETRY_LOG_PREFIX: &str = "tui_mailbox_telemetry=";
const TUI_FIRST_TOKEN_TELEMETRY_LOG_PREFIX: &str = "tui_first_token_telemetry=";
const TUI_WORKSPACE_INSTRUCTION_LOAD_WARNING_PREFIX: &str =
    "Agent TUI 工作区指令加载失败，已跳过：";

pub(super) use catalog::load_tui_catalog_snapshot;
use catalog::resolve_agent_tui_catalog;
use config_status::resolve_agent_tui_config_status;
use runtime_adapter::CliAgentTuiRuntimeAdapter;
use settings_persistence::persist_tui_settings_change;
pub(crate) use tui_submit_mapper::{TUI_CHANNEL, new_tui_reply_destination};

pub(crate) fn run_agent_tui_entry(cli: &AxonCli, prompt: Option<&str>) -> CliRunResult {
    let cwd = resolve_agent_tui_cwd(cli);
    let config_path = resolved_config_path(cli.global.config.as_deref()).ok();
    let axon_root = config_path
        .as_deref()
        .and_then(|path| resolved_axon_root(path).ok());
    if let StartupUpdateGate::Exit(result) =
        run_startup_update_gate(axon_root.as_deref(), prompt, cli.global.format)
    {
        return result;
    }
    let runtime_tmp_dir = resolve_agent_tui_tmp_dir(cli);
    let _ = cleanup_agent_tui_tmp_dir(&runtime_tmp_dir);
    ensure_agent_tui_tmp_cleanup_started(runtime_tmp_dir.clone());
    let overrides = resolve_agent_tui_run_overrides(cli);
    let workspace_instruction_paths = resolve_agent_tui_workspace_instruction_paths(&cwd);
    let request = AgentTuiLaunchRequest {
        cwd: cwd.clone(),
        prompt: prompt.map(ToOwned::to_owned),
        workspace_instruction_paths,
        config_status: resolve_agent_tui_config_status(cli),
        overrides,
        catalog: resolve_agent_tui_catalog(cli),
        runtime_tmp_dir,
    };
    if cli.global.format == OutputFormat::Json {
        let mut result = json_result(&launch_agent_tui_report(&request));
        result.exit_code = 0;
        return result;
    }
    let adapter = CliAgentTuiRuntimeAdapter {
        config_path: config_path.clone(),
        reply_destination: new_tui_reply_destination(),
    };
    match axon_tui_runtime::run_agent_tui_cli_with_runtime_adapter_and_settings(
        &request,
        adapter,
        move |change| {
            if let Some(path) = config_path.as_deref()
                && let Err(error) = persist_tui_settings_change(path, &change)
            {
                axon_logger::error(&format!("Agent TUI 设置写回失败：{error}"));
            }
        },
    ) {
        Ok(report) => {
            log_tui_mailbox_telemetry(&report);
            log_tui_first_token_telemetry(&report);
            CliRunResult {
                exit_code: 0,
                stdout: render_tui_exit_summary(&report),
                stderr: String::new(),
            }
        }
        Err(error) => CliRunResult {
            exit_code: error.exit_code(),
            stdout: String::new(),
            stderr: format!("{}\n", error.message()),
        },
    }
}

fn log_tui_mailbox_telemetry(report: &axon_tui_runtime::AgentTuiLaunchReport) {
    if !report.telemetry.mailbox.enabled {
        return;
    }
    match serde_json::to_string(&report.telemetry.mailbox) {
        Ok(snapshot) => {
            axon_logger::debug(&format!("{TUI_MAILBOX_TELEMETRY_LOG_PREFIX}{snapshot}"))
        }
        Err(error) => axon_logger::debug(&format!("TUI mailbox 遥测序列化失败: {error}")),
    }
}

fn log_tui_first_token_telemetry(report: &axon_tui_runtime::AgentTuiLaunchReport) {
    if !report.telemetry.first_token.enabled {
        return;
    }
    match serde_json::to_string(&report.telemetry.first_token) {
        Ok(snapshot) => {
            axon_logger::debug(&format!("{TUI_FIRST_TOKEN_TELEMETRY_LOG_PREFIX}{snapshot}"))
        }
        Err(error) => axon_logger::debug(&format!("TUI 首字遥测序列化失败: {error}")),
    }
}

pub(super) fn run_output_error_message(output: &axon_tui_runtime::AgentTuiRunOutput) -> String {
    output
        .lines()
        .iter()
        .find_map(|line| match line {
            axon_tui_runtime::AgentTuiRunLine::Error(error) => Some(error.message.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "runtime command failed".to_owned())
}

fn render_tui_exit_summary(report: &axon_tui_runtime::AgentTuiLaunchReport) -> String {
    if !report.session_summary.has_activity() {
        return String::new();
    }
    format!(
        "Axon Agent 会话完成：{}\n",
        report.session_summary.render_text()
    )
}

fn resolve_agent_tui_run_overrides(cli: &AxonCli) -> AgentTuiRunOverrides {
    let Some((model, reasoning_effort)) = resolved_config_path(cli.global.config.as_deref())
        .ok()
        .and_then(|path| ConfigLoader::load_from_path(&path).ok())
        .and_then(|loaded| default_tui_model_selection(&loaded.config))
    else {
        return AgentTuiRunOverrides::default();
    };
    AgentTuiRunOverrides {
        model: Some(model),
        reasoning_effort,
        ..AgentTuiRunOverrides::default()
    }
}

fn default_tui_model_selection(
    config: &axon_config::AxonConfig,
) -> Option<(String, Option<String>)> {
    let default_model = config.agent.default_model.as_str();
    let model = config
        .models
        .iter()
        .filter(|model| config.model_is_enabled(model))
        .find(|model| model.name == default_model)
        .or_else(|| {
            config
                .models
                .iter()
                .filter(|model| config.model_is_enabled(model))
                .find(|model| model.id == default_model)
        })
        .or_else(|| {
            config
                .models
                .iter()
                .find(|model| config.model_is_enabled(model))
        })?;
    Some((
        model.name.clone(),
        Some(config.agent.reasoning_effort.clone()),
    ))
}

fn resolve_agent_tui_cwd(cli: &AxonCli) -> PathBuf {
    cli.global
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn resolve_agent_tui_tmp_dir(cli: &AxonCli) -> PathBuf {
    resolved_config_path(cli.global.config.as_deref())
        .ok()
        .and_then(|path| resolved_axon_root(&path).ok())
        .map_or_else(
            || PathBuf::from(".axon").join("data").join("tmp"),
            |root| root.join("data").join("tmp"),
        )
}

fn resolve_agent_tui_workspace_instruction_paths(cwd: &Path) -> Vec<String> {
    match load_workspace_instructions(cwd) {
        Ok(instructions) => instructions
            .into_iter()
            .map(|instruction| instruction.path)
            .collect(),
        Err(error) => {
            axon_logger::warning(&format!(
                "{TUI_WORKSPACE_INSTRUCTION_LOAD_WARNING_PREFIX}{error}"
            ));
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_tui_runtime::{
        AgentTuiCatalog, AgentTuiConfigStatus, AgentTuiLaunchReport, AgentTuiSessionSummary,
        AgentTuiTokenUsage, TuiTelemetrySnapshot,
    };

    #[test]
    fn workspace_instruction_paths_follow_model_loading_order() {
        let root = axon_test_support::test_temp_dir();
        let child = root.path().join("crates").join("feature");
        std::fs::create_dir_all(root.path().join(".git")).expect("Git 标记目录应可创建");
        std::fs::create_dir_all(&child).expect("子工作区应可创建");
        std::fs::write(root.path().join("AGENTS.md"), "根目录规则").expect("根目录指令应可写入");
        std::fs::write(child.join("AGENTS.md"), "子目录规则").expect("子目录指令应可写入");

        let paths = resolve_agent_tui_workspace_instruction_paths(&child);

        // 契约：祖先目录的指令报绝对路径，就在 cwd 里的报相对路径。
        // Windows 的 `\\?\` 前缀一度让 strip_prefix 落空，同一条用例在两平台上报出不同值。
        let expected_root_path = root
            .path()
            .canonicalize()
            .expect("临时工作区应可规范化")
            .join("AGENTS.md")
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(paths, vec![expected_root_path, "AGENTS.md".to_owned()]);
    }

    #[test]
    fn workspace_instruction_path_loading_failure_is_skipped() {
        let root = axon_test_support::test_temp_dir();
        let missing = root.path().join("missing");

        let paths = resolve_agent_tui_workspace_instruction_paths(&missing);

        assert!(paths.is_empty());
    }

    #[test]
    fn render_tui_exit_summary_skips_empty_session() {
        let report = launch_agent_tui_report(&AgentTuiLaunchRequest {
            cwd: std::path::PathBuf::from("/workspace"),
            prompt: None,
            workspace_instruction_paths: Vec::new(),
            config_status: AgentTuiConfigStatus::new("openai", "gpt-4o", "已认证"),
            overrides: AgentTuiRunOverrides::default(),
            catalog: AgentTuiCatalog::default(),
            runtime_tmp_dir: std::path::PathBuf::from("/workspace/.axon/data/tmp"),
        });

        assert!(render_tui_exit_summary(&report).is_empty());
    }

    #[test]
    fn render_tui_exit_summary_projects_session_activity() {
        let summary = AgentTuiSessionSummary {
            session_id: Some("session-2".to_owned()),
            title: Some("会话标题".to_owned()),
            resumed_from: Some("session-1".to_owned()),
            turns: 1,
            failed_turns: 0,
            tool_starts: 2,
            tool_output_chunks: 0,
            token_usage: AgentTuiTokenUsage::new(10, 2, 0, 3, 1),
            elapsed_ms: 125_000,
        };
        let baseline_report = launch_agent_tui_report(&AgentTuiLaunchRequest {
            cwd: std::path::PathBuf::from("/workspace"),
            prompt: None,
            workspace_instruction_paths: Vec::new(),
            config_status: AgentTuiConfigStatus::new("openai", "gpt-4o", "已认证"),
            overrides: AgentTuiRunOverrides::default(),
            catalog: AgentTuiCatalog::default(),
            runtime_tmp_dir: std::path::PathBuf::from("/tmp/axon"),
        });
        let report = AgentTuiLaunchReport {
            status: axon_tui_runtime::AGENT_TUI_STATUS_SUBMITTED,
            kind: "agent",
            cwd: "/workspace".to_owned(),
            prompt: None,
            submitted_prompt: Some("修复构建".to_owned()),
            session_usage: summary.token_usage,
            session_summary: summary,
            panel_title: "Axon Agent",
            focus_target: "agent_composer",
            prompt_indicator: "›",
            config_status: AgentTuiConfigStatus::new("openai", "gpt-4o", "已认证"),
            overrides: AgentTuiRunOverrides::default(),
            telemetry: TuiTelemetrySnapshot::default(),
            terminal_compatibility: axon_tui_runtime::TerminalCompatibilitySnapshot::default(),
            ability_matrix: baseline_report.ability_matrix,
            smoke_contract: baseline_report.smoke_contract,
            message: "Axon Agent TUI 已关闭".to_owned(),
        };

        assert_eq!(
            render_tui_exit_summary(&report),
            "Axon Agent 会话完成：轮次 1 · session=session-2 · 标题=会话标题 · 恢复自=session-1 · 工具 2 · 用量 16 · 耗时=2m 05s\n"
        );
    }
}
