use super::*;
use crate::management_commands::{cli_config_error, cli_error, json_result, resolved_config_path};
use axon_config::{AxonConfig, load_provider_credentials_from_env_file};
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
mod run_compaction;
mod run_compaction_notice;
mod run_compaction_projection;
mod run_completion_notice_subscription;
mod run_continuation_assembly;
mod run_execution;
mod run_failover;
mod run_file_change_projection;
mod run_json_wire_events;
mod run_mailbox_persistence;
mod run_memory_autonomy;
mod run_memory_citation;
mod run_memory_recall;
mod run_memory_surface;
mod run_memory_usefulness;
mod run_memory_write;
mod run_model_payload;
pub(crate) mod run_model_routing;
mod run_outcome_diagnostics;
mod run_persona_settlement;
mod run_plan_projection;
mod run_post_run_governance;
mod run_post_run_governance_performance;
mod run_post_run_governance_store;
mod run_post_run_governance_worker;
mod run_progress;
mod run_progress_apply_patch_snapshot;
mod run_progress_hashline_snapshot;
mod run_progress_streaming_diff_preview;
mod run_provider_loop;
mod run_provider_orchestration;
mod run_question_projection;
pub(crate) mod run_remote_execution;
mod run_request;
mod run_request_build_options;
mod run_request_builder;
mod run_request_payload;
mod run_request_session;
pub(crate) mod run_request_subagents;
mod run_rollout_store;
mod run_self_iteration_subagent;
mod run_session_store;
mod run_skill_evolution;
pub(crate) mod run_skill_evolution_materialize;
#[cfg(test)]
mod run_skill_evolution_post_run_tests;
pub(crate) mod run_skill_evolution_store;
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
mod run_thread_history;
mod run_thread_store;
mod run_thread_title;
mod run_tool_loop;
mod run_tool_pool;
mod run_tool_result_cache;
mod run_turn_outcome;
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
    PreparedInteraction, interaction_from_prompt, prepare_interaction_from_prompt,
    register_workspace, register_workspace_record,
};
#[cfg(feature = "tui")]
pub(crate) use interactive_tui::run_agent_tui_entry;
pub(crate) use run_agent_identity::RunAgentIdentity;
use run_execution::{
    ProviderStreamingRunner, RunExecutionError, RunJsonEvent, RunResponse, new_run_session_id,
    now_ms,
};
use run_json_wire_events::CompletedRunWireEventsInput;
use run_memory_autonomy::apply_memory_autonomy_feedback;
use run_memory_recall::RunMemoryRecallEvent;
use run_memory_usefulness::memory_usefulness_for_completed;
pub(crate) use run_memory_write::AgentMemorySearchInput;
use run_model_routing::{
    FAST_MODEL_SLOT_NAME, select_channel_balanced_model,
    select_channel_balanced_model_for_agent_slot,
};
use run_post_run_governance::{
    PostRunGovernanceJob, PostRunGovernanceReceipt, start_post_run_governance_job,
};
pub(crate) use run_post_run_governance_worker::run_governance_worker;
pub(crate) use run_progress::question_part_id;
pub(crate) use run_provider_orchestration::{RunProgressOptions, run_run_with_progress};
pub(crate) use run_question_projection::run_lane_id;
pub(crate) use run_question_projection::{
    DurableQuestionEventSlot, DurableQuestionEventWriter,
    production_question_event_sink_with_durable_slot,
};
pub(crate) use run_remote_execution::{RemoteAgentExecutionInput, enqueue_remote_gateway_alert};
use run_request::RunRequest;
use run_request_build_options::RunRequestBuildOptions;
use run_request_session::RunRequestSession;
pub(crate) use run_request_subagents::execute_standalone_subagent;
use run_rollout_store::commit_session_to_rollout;
pub(crate) use run_rollout_store::select_rollout_session;
#[cfg(test)]
pub(crate) use run_rollout_store::{RolloutEventAppendInput, append_rollout_event_for_session};
pub(crate) use run_rollout_store::{
    RolloutEventRecord, RolloutReplayView, RolloutSqliteStore, read_rollout_tool_output,
    replay_rollout_events, replay_session_chain, resolved_rollout_db_path,
};
pub(crate) use run_session_store::RunSessionRecord;
pub(crate) use run_session_store::resolved_axon_root;
pub(crate) use run_startup_snapshot::RunStartupSnapshot;
pub(crate) use run_startup_snapshot_cache::StartupSnapshotCache;
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
    let interaction = match interaction_from_prompt(
        CLI_INTERACTION_CHANNEL,
        cli_reply_destination(),
        &command.prompt,
        &cwd,
        axon_time::unix_now_ms(),
    ) {
        Ok(interaction) => interaction,
        Err(error) => return cli_error(error),
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

pub(crate) fn load_thread_session(
    config_path: &Path,
    session_id: Option<&str>,
) -> Result<Option<RunSessionRecord>, String> {
    run_thread_store::load_thread_session(config_path, session_id)
}

use run_thread_store::{
    mirror_failed_session_to_thread, mirror_session_started, mirror_session_to_thread,
    prepare_run_session,
};

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

struct CompletedRun {
    response: RunResponse,
    session_title: Option<String>,
    memory_usefulness: Option<run_memory_usefulness::RunMemoryUsefulnessEvent>,
    memory_autonomy: Option<axon_agent_runtime::MemoryAutonomyFeedbackReport>,
    skill_evolution: Option<run_skill_evolution::SkillEvolutionEvent>,
    post_run_governance: Option<PostRunGovernanceReceipt>,
}

fn finish_completed_run(
    config_path: &Path,
    request: &RunRequest,
    response: RunResponse,
) -> Result<CompletedRun, String> {
    let memory_usefulness = memory_usefulness_for_completed(request, &response);
    let record = RunSessionRecord::from_completed(
        request,
        &response,
        memory_usefulness.as_ref(),
        &request.startup.workspace_instructions,
    );
    let session_title = run_thread_title::derive_thread_title(&record.prompt);
    commit_session_to_rollout(config_path, &record, &session_title)?;
    if let Err(error) = mirror_session_to_thread(config_path, &record) {
        axon_logger::warning(&format!(
            "thread projection skipped: session_id={}, error={error}",
            record.session_id
        ));
    } else if let Err(error) =
        run_thread_title::mirror_thread_title(config_path, &record, &session_title)
    {
        axon_logger::warning(&format!(
            "thread title projection skipped: session_id={}, error={error}",
            record.session_id
        ));
    }
    let governance_job = PostRunGovernanceJob {
        config_path: config_path.to_path_buf(),
        request: request.clone(),
        session_record: record,
        memory_usefulness: memory_usefulness.clone(),
    };
    let post_run_governance = Some(start_post_run_governance_job(&governance_job));
    Ok(CompletedRun {
        response,
        session_title: Some(session_title),
        memory_usefulness,
        memory_autonomy: None,
        skill_evolution: None,
        post_run_governance,
    })
}

fn finish_failed_run(
    config_path: &Path,
    config: &AxonConfig,
    request: &RunRequest,
    error: &RunExecutionError,
    output: OutputFormat,
    progress: Option<&run_progress::RunProgressSink<'_>>,
) -> CliRunResult {
    let partial_response =
        progress.map_or_else(String::new, |sink| sink.partial_assistant_content());
    let timestamp = run_execution::now_ms();
    // 失败轮不入治理队列，因此人格结算在这里直接完成，保证失败转移在生产路径可达。
    // 用户中断与真正的失败必须分开记账：前者按「本轮被中断」，后者才累加失败连击。
    let final_status = if request
        .cancel_token
        .as_ref()
        .is_some_and(axon_agent_runtime::RunCancelToken::is_cancelled)
    {
        axon_persona::TurnFinalStatus::Interrupted
    } else {
        axon_persona::TurnFinalStatus::Failed
    };
    let part_id = format!("failed:{timestamp}");
    run_persona_settlement::settle_persona_turn(&run_persona_settlement::PersonaSettlementInput {
        config_path,
        session_id: &request.session_id,
        assistant_part_id: &part_id,
        disclosure: request.goal_disclosure_policy,
        final_status,
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        scene_audience: None,
    });
    let failure = run_session_store::runtime_failure_from_execution_error(
        &request.session_id,
        error,
        &partial_response,
        timestamp,
    );
    if let Err(persistence_error) =
        persist_failed_run(config_path, request, error, partial_response, &failure)
    {
        axon_logger::warning(&format!(
            "failed run persistence skipped: session_id={}, error={persistence_error}",
            request.session_id
        ));
    }
    let memory_usefulness = run_memory_usefulness::memory_usefulness_for_failed(request, error);
    let memory_autonomy = apply_memory_autonomy_feedback(
        config_path,
        config,
        request,
        memory_usefulness.as_ref(),
        false,
    );
    let report = RunJsonEvent::failed(
        request,
        error,
        &failure,
        memory_usefulness.as_ref(),
        memory_autonomy.as_ref(),
    );
    match output {
        OutputFormat::Text => CliRunResult {
            exit_code: 1,
            stdout: super::run_commands::run_text_render::render_text_run_failure(request, error),
            stderr: String::new(),
        },
        OutputFormat::Json => {
            let mut result = json_result(&report);
            result.exit_code = 1;
            result
        }
    }
}

fn persist_failed_run(
    config_path: &Path,
    request: &RunRequest,
    error: &RunExecutionError,
    partial_response: String,
    failure: &axon_domain::RuntimeFailure,
) -> Result<(), String> {
    let started = RunSessionRecord::started(request);
    let persisted = load_thread_session(config_path, Some(&request.session_id))?;
    let mut record = select_failed_run_record(started, persisted);
    record.mark_failed(failure.clone(), partial_response, error);
    let title = run_thread_title::derive_thread_title(&record.prompt);
    commit_session_to_rollout(config_path, &record, &title)?;
    mirror_failed_session_to_thread(config_path, &record, failure)
}

fn select_failed_run_record(
    started: RunSessionRecord,
    persisted: Option<RunSessionRecord>,
) -> RunSessionRecord {
    persisted
        .filter(|record| record.assistant_part_id == started.assistant_part_id)
        .unwrap_or(started)
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
