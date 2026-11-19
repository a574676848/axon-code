use super::*;
use crate::management_commands::{cli_config_error, cli_error, json_result, resolved_config_path};
use axon_config::load_provider_credentials_from_env_file;
use std::io::Write as IoWrite;
use std::path::Path;
use std::sync::OnceLock;

mod daemon_completion_inbox_client;
mod daemon_event_cursor;
mod daemon_event_subscription_client;
mod daemon_history_loader;
mod daemon_interaction_client;
#[cfg(test)]
mod daemon_interaction_source_guard_tests;
mod daemon_interaction_submission;
mod daemon_subscription_recovery;
mod interaction_adapter;
#[cfg(feature = "tui")]
mod interactive_tui;
#[cfg(test)]
#[path = "question_answer_wake_tests.rs"]
mod question_answer_wake_tests;
mod run_agent_identity;
mod run_agent_store;
mod run_capacity;
pub(crate) mod run_channel_affinity;
mod run_compaction_projection;
mod run_completion_notice_subscription;
mod run_continuation_assembly;
mod run_environment_context;
mod run_execution;
mod run_execution_stop_watch;
mod run_failover;
mod run_file_change_projection;
mod run_identities;
pub(crate) mod run_job_lifecycle_events;
mod run_json_wire_events;
mod run_mailbox_persistence;
mod run_memory_autonomy;
mod run_memory_citation;
mod run_memory_recall;
mod run_memory_surface;
mod run_memory_thread_context;
mod run_memory_usefulness;
mod run_memory_write;
mod run_model_payload;
pub(crate) mod run_model_routing;
mod run_outcome_diagnostics;
mod run_persona_settlement;
mod run_plan_projection;
mod run_post_run_artifacts;
mod run_post_run_governance;
mod run_post_run_governance_performance;
mod run_post_run_governance_store;
mod run_post_run_governance_worker;
#[cfg(test)]
mod run_post_run_reflection_tests;
mod run_progress;
mod run_progress_apply_patch_snapshot;
mod run_progress_hashline_snapshot;
pub(crate) use run_identities::session_identities;
use run_identities::{RunIdentities, resolve_run_identities};

/// 测试用：一对同源的会话身份与运行窗口。
#[cfg(test)]
pub(crate) fn unique_test_run_identities() -> RunIdentities {
    let id = run_execution::new_run_session_id();
    RunIdentities::new(id.clone(), id)
}
mod run_progress_streaming_diff_preview;
mod run_provider_loop;
mod run_provider_turn;
pub(crate) use run_provider_turn::CliProviderTurnRunner;
mod run_cli_surface;
mod run_provider_orchestration;
mod run_question_projection;
mod run_reflection_apply;
mod run_reflection_batch;
mod run_request;
mod run_request_build_options;
mod run_request_builder;
mod run_request_payload;
mod run_request_session;
pub(crate) mod run_request_subagents;
pub(crate) mod run_resume_pointer;
mod run_self_iteration_subagent;
mod run_semantic_salience;
pub(crate) mod run_session_store;
mod run_session_title;
pub(crate) mod run_shell_job_wake;
mod run_startup_snapshot;
mod run_startup_snapshot_cache;
mod run_startup_snapshot_fingerprint;
pub(crate) mod run_steer;
mod run_stop_control;
mod run_stream_event_batch;
mod run_stream_provider_progress;
mod run_subagent_completion;
pub(crate) mod run_subagent_executor;
pub(crate) mod run_subagent_executor_support;
mod run_subagent_lifecycle;
mod run_subagent_performance;
mod run_subagent_progress;
mod run_text_render;
pub(crate) mod run_thread_children;
mod run_thread_history;
mod run_thread_partial;
pub(crate) mod run_thread_replay;
pub(crate) mod run_thread_store;
mod run_thread_tail;
mod run_thread_title;
mod run_tool_loop;
mod run_tool_pool;
mod run_tool_result_cache;
mod run_turn_accounting;
mod run_turn_execution;
mod run_turn_metrics;
mod run_turn_outcome;
mod run_turn_spec;
mod run_turn_start;
mod run_turn_terminal;
pub(crate) mod run_workspace_identity;
pub(crate) use run_tool_result_cache::workspace_revision;
mod run_tool_result_wire;
mod run_tool_write_coordination;

#[cfg(test)]
#[path = "run_tool_benchmark_tests.rs"]
mod run_tool_benchmark_tests;

pub(crate) use axon_agent_runtime::RunCancelToken;
pub(crate) use daemon_interaction_client::{execute_daemon_interaction, run_daemon_interaction};
pub(crate) use daemon_interaction_submission::{
    submit_daemon_interaction, submit_prepared_daemon_interaction,
};
pub(crate) use interaction_adapter::{
    PreparedInteraction, interaction_store, prepare_background_interaction,
    prepare_interaction_from_prompt_as, register_workspace, register_workspace_record,
};
#[cfg(feature = "tui")]
pub(crate) use interactive_tui::run_agent_tui_entry;
pub(crate) use run_agent_identity::RunAgentIdentity;
use run_execution::{RunJsonEvent, now_ms};
use run_json_wire_events::CompletedRunWireEventsInput;
use run_memory_recall::RunMemoryRecallEvent;
pub(crate) use run_memory_write::AgentMemorySearchInput;
use run_model_routing::{
    FAST_MODEL_SLOT_NAME, select_channel_balanced_model,
    select_channel_balanced_model_for_agent_slot,
};
pub(crate) use run_post_run_governance_worker::run_governance_worker;
pub(crate) use run_progress::question_part_id;
pub(crate) use run_provider_orchestration::{RunProgressOptions, run_run_with_progress};
pub(crate) use run_question_projection::{
    DurableQuestionEventSlot, DurableQuestionEventWriter,
    production_question_event_sink_with_durable_slot,
};
use run_request::RunRequest;
use run_request_build_options::RunRequestBuildOptions;
use run_request_session::RunRequestSession;
pub(crate) use run_request_subagents::execute_standalone_subagent;
pub(crate) use run_resume_pointer::select_resume_session;
pub(crate) use run_resume_pointer::{restore_resume_session, snapshot_resume_session};
pub(crate) use run_session_store::RunSessionRecord;
pub(crate) use run_session_store::resolved_axon_root;
pub(crate) use run_startup_snapshot::RunStartupSnapshot;
pub(crate) use run_startup_snapshot_cache::{StartupSnapshotCache, shared_startup_snapshot_cache};
pub(crate) use run_stop_control::{RunStopControl, cleanup_stale_run_controls, request_stop};
pub(crate) const CLI_INTERACTION_CHANNEL: &str = "cli";

/// 当前 CLI 进程的会话表面 ID。
///
/// 每次 `axon run` 都是独立进程，因此每个进程独占一条主 turn 车道：同一 Workspace 下
/// 并发的 CLI 调用不会再把输入并入同一个 primary interaction（那会让两个前台争用同一
/// interaction 的事件流，导致答复错位或订阅等待失败）。需要延续同一段上下文时，用
/// `--continue` 或 `--session` 显式指定会话。
pub(crate) fn cli_reply_destination() -> &'static str {
    static DESTINATION: OnceLock<String> = OnceLock::new();
    DESTINATION.get_or_init(|| format!("terminal-{}", uuid::Uuid::new_v4()))
}

pub(crate) fn run_cli_daemon_interaction(
    cli: &AxonCli,
    command: &RunCommand,
    live_writer: Option<&mut dyn IoWrite>,
) -> CliRunResult {
    if command.prompt.trim().is_empty() {
        return cli_error("Interaction 内容不能为空");
    }
    let cwd = cli
        .global
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let interaction = {
        // 显式续接必须复用目标会话的渠道与投递目标；未指定会话时新建一条主会话，
        // 投递目标只决定投递，不参与会话身份。
        let surface = match run_cli_surface::resolve_cli_run_surface(
            cli.global.config.as_deref(),
            command.session.as_deref(),
            command.continue_last,
            &cwd,
        ) {
            Ok(surface) => surface,
            Err(error) => return cli_error(&error),
        };
        let target_session = match surface.session_id.as_deref() {
            Some(value) => match axon_domain::DaemonSessionId::new(value) {
                Ok(session_id) => Some(session_id),
                Err(error) => return cli_error(format!("会话标识非法：{error}")),
            },
            None => None,
        };
        let prepared =
            match crate::run_commands::interaction_adapter::prepare_interaction_from_prompt_as(
                target_session.as_ref(),
                &cwd,
                cli.global.config.as_deref(),
                &surface.channel,
                &surface.destination,
                &command.prompt,
                axon_time::unix_now_ms(),
                Some("cli"),
            ) {
                Ok(prepared) => prepared,
                Err(error) => return cli_error(error),
            };
        prepared.interaction
    };
    run_daemon_interaction(
        interaction,
        &cwd,
        cli.global.config.as_deref(),
        command.format.unwrap_or(cli.global.format),
        command.timeout,
        &|| false,
        None,
        live_writer,
    )
}

use run_thread_store::{mirror_session_started, prepare_run_session};

pub(crate) fn run_planned_invocation(cli: &AxonCli) -> CliRunResult {
    match plan_invocation(cli) {
        CliInvocation::NoCommand { prompt } => run_default_entry(cli, prompt.as_deref()),
        CliInvocation::Command { name } => match cli.global.format {
            OutputFormat::Text => CliRunResult {
                exit_code: 2,
                stdout: String::new(),
                stderr: format!(
                    "axon {name} 已完成参数解析；该运行时将在对应 AXON 任务完成后启用\n"
                ),
            },
            OutputFormat::Json => {
                let mut result = json_result(&CliRuntimePendingReport::command(name));
                result.exit_code = 2;
                result
            }
        },
    }
}

fn run_default_entry(cli: &AxonCli, prompt: Option<&str>) -> CliRunResult {
    #[cfg(feature = "tui")]
    {
        run_agent_tui_entry(cli, prompt)
    }
    #[cfg(not(feature = "tui"))]
    {
        if let Some(prompt) = prompt.filter(|value| !value.trim().is_empty()) {
            let command = RunCommand {
                prompt: prompt.to_owned(),
                model: None,
                reasoning_effort: None,
                format: Some(cli.global.format),
                continue_last: false,
                session: None,
                file: Vec::new(),
                timeout: None,
                tool: Vec::new(),
            };
            return run_cli_daemon_interaction(cli, &command, None);
        }
        no_command_result()
    }
}

#[cfg(not(feature = "tui"))]
fn no_command_result() -> CliRunResult {
    let mut command = AxonCli::command();
    let mut buffer = Vec::new();
    let _ = command.write_long_help(&mut buffer);
    let mut stdout = String::from_utf8(buffer).unwrap_or_default();
    stdout.push_str("\n\n当前构建未启用 tui。请使用 `axon run \"<prompt>\"`。\n");
    CliRunResult {
        exit_code: 0,
        stdout,
        stderr: String::new(),
    }
}

pub(crate) fn load_run_env_file(path: &Path) -> Result<(), String> {
    let path_display = path.display();
    axon_logger::debug(&format!("load_run_env_file: 开始加载 {path_display}"));

    let loaded_count = load_provider_credentials_from_env_file(path)?;
    if !path.exists() {
        axon_logger::debug(&format!("load_run_env_file: 文件不存在 {path_display}"));
        return Ok(());
    }

    axon_logger::debug(&format!(
        "load_run_env_file: 成功加载 {loaded_count} 个环境变量"
    ));
    Ok(())
}

#[cfg(test)]
mod env_file_tests {
    use super::load_run_env_file;
    use axon_config::provider_credential;

    #[test]
    fn load_run_env_file_populates_provider_credential_store() {
        let path = axon_test_support::test_temp_root().join(format!(
            "axon-env-{}-{}.env",
            std::process::id(),
            "credential-store"
        ));
        std::fs::write(&path, "AXON_TEST_ENV_STORE='secret-value'\n").unwrap();

        load_run_env_file(&path).unwrap();

        assert_eq!(
            provider_credential("AXON_TEST_ENV_STORE").as_deref(),
            Some("secret-value")
        );
        let _ = std::fs::remove_file(path);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CliRuntimePendingReport {
    status: &'static str,
    kind: &'static str,
    command: Option<&'static str>,
    prompt: Option<String>,
    message: String,
}

impl CliRuntimePendingReport {
    fn command(command: &'static str) -> Self {
        Self {
            status: "runtime_pending",
            kind: "command",
            command: Some(command),
            prompt: None,
            message: format!("axon {command} 运行时将在对应 AXON 任务完成后启用"),
        }
    }
}

pub(crate) use run_memory_write::{AgentMemoryWriteInput, AgentMemoryWriteRuntime};
pub(crate) use run_turn_metrics::{publish_turn_metrics, register_process_host};
