use super::{
    CliRunResult, CompletedRun, CompletedRunWireEventsInput, ConfigLoader, OutputFormat,
    ProviderStreamingRunner, RunAgentIdentity, RunCancelToken, RunCommand, RunJsonEvent,
    RunRequest, RunRequestBuildOptions, RunRequestSession, RunResponse, RunSessionRecord,
    RunStartupSnapshot, RunStopControl, cli_config_error, cli_error, finish_completed_run,
    finish_failed_run, json_result, load_run_env_file, mirror_session_started, new_run_session_id,
    prepare_run_session, resolved_axon_root, resolved_config_path, run_compaction,
    run_compaction_notice, run_execution, run_outcome_diagnostics, run_progress,
    run_request_payload::RunWorkspaceSnapshotPrewarm, run_steer,
};
use axon_agent_runtime::{AgentPath, ShellCommandToolRuntime};
use axon_client_identity::ClientChannel;
use axon_domain::QuotedMessageContext;
use axon_session_runtime::{
    SubagentSessionSinkFactory, TurnPerformanceRecorder, TurnPerformanceStage,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub(crate) struct RunProgressOptions<'a> {
    pub(crate) client_channel: ClientChannel,
    pub(crate) cancel_token: Option<RunCancelToken>,
    pub(crate) progress: Option<&'a mut dyn FnMut(run_progress::RunProgressWireEvent)>,
    pub(crate) subagent_event_bus: Option<run_progress::RunProgressEventBus>,
    /// 宿主提供的子代理会话 sink 工厂；缺省表示本轮不持久化子代理逐字稿。
    pub(crate) subagent_session_sinks: Option<Arc<dyn SubagentSessionSinkFactory>>,
    pub(crate) startup_snapshot: Option<Arc<RunStartupSnapshot>>,
    pub(crate) steer_inbox: Option<run_steer::RunSteerInbox>,
    /// 主 turn 专属的用户提问端口；缺省表示本轮不允许向用户提问。
    pub(crate) user_question_port: Option<std::sync::Arc<dyn axon_agent_runtime::UserQuestionPort>>,
    pub(crate) quoted_message: Option<QuotedMessageContext>,
    /// 原 Interaction 的运行时投递上下文，不接受模型覆盖。
    pub(crate) transport_context: Option<serde_json::Value>,
    pub(crate) external_stop_control: bool,
    /// 本次运行的身份；默认是用户可见的主代理 turn。
    pub(crate) agent_identity: RunAgentIdentity,
    pub(crate) performance: Option<Arc<dyn TurnPerformanceRecorder>>,
}

impl Default for RunProgressOptions<'_> {
    fn default() -> Self {
        Self {
            client_channel: ClientChannel::Cli,
            cancel_token: None,
            progress: None,
            subagent_event_bus: None,
            subagent_session_sinks: None,
            startup_snapshot: None,
            steer_inbox: None,
            user_question_port: None,
            quoted_message: None,
            transport_context: None,
            external_stop_control: false,
            agent_identity: RunAgentIdentity::PrimaryPrivate,
            performance: None,
        }
    }
}

pub(crate) fn run_run_with_progress(
    command: &RunCommand,
    config_path: Option<&Path>,
    cwd: Option<&Path>,
    options: RunProgressOptions<'_>,
) -> CliRunResult {
    let path = match prepare_run_environment(command, config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let workspace_snapshot_prewarm = cwd.map(|cwd| {
        RunWorkspaceSnapshotPrewarm::spawn(
            path.clone(),
            cwd.to_path_buf(),
            options.performance.clone(),
        )
    });
    // 隐式入口（未指定 --continue/--session）先解析当前 CLI 复合车道：已有 Session
    // 直接加载，未建才创建，使同表面相邻输入延续同一段上下文。显式入口与子代理不受影响。
    let implicit_session_id = match (command.continue_last, command.session.as_deref(), cwd) {
        (false, None, Some(cwd)) => {
            match super::interaction_adapter::conversation_lane_run_session_id(
                cwd,
                Some(&path),
                run_execution::now_ms(),
                super::CLI_INTERACTION_CHANNEL,
                super::cli_reply_destination(),
            ) {
                Ok(session_id) => Some(session_id),
                Err(error) => return cli_error(error),
            }
        }
        _ => None,
    };
    let previous_session = match prepare_run_session(&path, command, implicit_session_id.as_deref())
    {
        Ok(session) => session,
        Err(error) => return cli_error(error),
    };
    if let Some(performance) = &options.performance {
        performance.record(TurnPerformanceStage::HistoryReady);
    }
    let session_id = match (
        previous_session
            .as_ref()
            .map(|session| session.record.session_id.clone()),
        command.session.clone(),
        implicit_session_id,
    ) {
        (Some(session_id), _, _) => session_id,
        (None, Some(session_id), _) => session_id,
        // 未显式指定 Session 时收敛到当前 CLI 复合车道：已建则复用同一个运行窗口，
        // 未建则刚以同一个稳定身份建档。不同会话表面仍相互隔离，不共享 Provider history。
        (None, None, Some(session_id)) => session_id,
        (None, None, None) => new_run_session_id(),
    };
    run_compaction::cancel_idle_compaction(&session_id);
    let cancel_token = run_cancel_token(&options);
    let progress = options.progress.map(|emit| {
        let sink = run_progress::RunProgressSink::with_session_id(emit, session_id.clone());
        // 超预算的工具结果全文落在运行态数据根下，事件里只带路径。
        match crate::runtime_paths::runtime_axon_data_path_static(&path) {
            Ok(root) => sink.with_tool_result_archive_root(root),
            Err(_) => sink,
        }
    });
    // 空闲轮转在本 turn 收尾之后执行，那时本通道已经关闭；结果在这里补投。
    if let Some(sink) = progress.as_ref()
        && let Ok(root) = resolved_axon_root(&path)
    {
        run_compaction_notice::project_pending_compaction_notice(Some(sink), &root, &session_id);
    }
    let loaded = match ConfigLoader::load_from_path(&path) {
        Ok(loaded) => loaded,
        Err(error) => return cli_config_error(&error),
    };
    if let Some(performance) = &options.performance {
        performance.record(TurnPerformanceStage::ConfigReady);
    }
    let output = command.format.unwrap_or(OutputFormat::Text);
    let recorded_cwd = previous_session
        .as_ref()
        .and_then(|session| session.record.cwd.as_deref())
        .map(Path::new);
    let effective_cwd = match super::run_request::resolve_effective_run_cwd(cwd, recorded_cwd) {
        Ok(cwd) => cwd,
        Err(error) => return cli_error(error),
    };
    let startup = match RunStartupSnapshot::resolve_for_run(
        &path,
        Some(&effective_cwd),
        options.startup_snapshot,
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => return cli_error(error),
    };
    if let Some(performance) = &options.performance {
        performance.record(TurnPerformanceStage::StartupReady);
    }
    let request_session = RunRequestSession::with_id(previous_session.as_ref(), session_id);
    let request_performance = options.performance.clone();
    let mut request = match RunRequest::from_command(
        command,
        &loaded.config,
        &path,
        request_session,
        Some(&effective_cwd),
        RunRequestBuildOptions {
            startup,
            cancel_token: cancel_token.clone(),
            subagent_event_bus: options.subagent_event_bus,
            subagent_session_sinks: options.subagent_session_sinks,
            agent_identity: options.agent_identity,
            client_channel: options.client_channel,
            performance: request_performance,
            workspace_snapshot_prewarm,
            transport_context: options.transport_context.clone(),
        },
    ) {
        Ok(request) => request,
        Err(error) => return cli_error(error),
    };
    if let Some(performance) = &request.performance {
        performance.record(TurnPerformanceStage::RunRequestBuilt);
    }
    request.steer_inbox = options.steer_inbox;
    // 用户提问端口是主 turn 专属能力：装配层只在这条路径上注入，
    // 子代理与后台执行链的 RunRequest 永远保持 None。
    request.user_question_port = options.user_question_port;
    // 子代理投递通道在本轮控制面就位时装上句柄：装上之前该通道一律拒绝投递，
    // 因此不存在「给子代理的话悄悄进了主代理上下文」的窗口。
    if let Some(inbox) = request.steer_inbox.as_ref() {
        inbox
            .subagent_channel()
            .install(Arc::clone(&request.agent_control));
    }
    request.quoted_message = options.quoted_message;
    request.goal_transport_context = options.transport_context;
    if let Err(error) = persist_cli_run_started(&request) {
        return cli_error(format!("Provider turn 起始持久化失败：{error}"));
    }
    if let Some(performance) = &request.performance {
        performance.record(TurnPerformanceStage::ThreadStartMirrorReady);
        performance.record(TurnPerformanceStage::RunStartedPersisted);
    }
    let _stop_control = match start_external_stop_control(
        options.external_stop_control,
        &path,
        &request,
        cancel_token,
    ) {
        Ok(control) => control,
        Err(error) => return cli_error(error),
    };
    let started_at_ms = run_execution::now_ms();
    let provider_result = ProviderStreamingRunner::run(&request, progress.as_ref());
    axon_logger::debug(&format!(
        "ProviderStreamingRunner 返回: session={}, elapsed_ms={}",
        request.session_id,
        run_execution::now_ms().saturating_sub(started_at_ms)
    ));
    cancel_all_running_subagents(&request.subagent_cancel_tokens);
    ShellCommandToolRuntime::terminate_sessions_for_owner(&request.session_id);
    // 父 turn 已结束：终态作业的查询消费者不复存在（结果只经完成主动通知交付），
    // 立即释放该 owner 的终态作业，不让后台保留窗拖住本轮交付链路。
    axon_agent_runtime::JobRuntime::global()
        .release_terminal_for_owner(&request.session_id, std::time::Instant::now());
    let teardown_started_ms = run_execution::now_ms();
    let run_result = match provider_result {
        Ok(response) => finish_successful_provider_run(
            &CompletedRunContext {
                path: &path,
                config: &loaded.config,
                request: &request,
                elapsed_ms: run_execution::now_ms().saturating_sub(started_at_ms),
                output,
                progress: progress.as_ref(),
            },
            response,
        ),
        Err(error) => finish_failed_run(
            &path,
            &loaded.config,
            &request,
            &error,
            output,
            progress.as_ref(),
        ),
    };
    axon_logger::debug(&format!(
        "run 收尾持久化完成: session={}, teardown_ms={}",
        request.session_id,
        run_execution::now_ms().saturating_sub(teardown_started_ms)
    ));
    run_result
}

fn cancel_all_running_subagents(cancel_tokens: &Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>) {
    let tokens = {
        let mut cancel_tokens = cancel_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *cancel_tokens)
    };
    for token in tokens.into_values() {
        token.cancel();
    }
}

fn prepare_run_environment(
    command: &RunCommand,
    config_path: Option<&Path>,
) -> Result<PathBuf, String> {
    if command.continue_last && command.session.is_some() {
        return Err("axon run 不能同时使用 --continue 和 --session".to_owned());
    }
    let path = resolved_config_path(config_path)?;
    load_run_env_file(&crate::setup_commands::env_path_for_config(&path))?;
    Ok(path)
}

fn run_cancel_token(options: &RunProgressOptions<'_>) -> Option<RunCancelToken> {
    options
        .cancel_token
        .clone()
        .or_else(|| options.external_stop_control.then(RunCancelToken::default))
}

fn start_external_stop_control(
    enabled: bool,
    config_path: &Path,
    request: &RunRequest,
    cancel_token: Option<RunCancelToken>,
) -> Result<Option<RunStopControl>, String> {
    if !enabled {
        return Ok(None);
    }
    RunStopControl::start(
        config_path,
        &request.session_id,
        cancel_token.expect("CLI stop 控制必须创建取消令牌"),
    )
    .map(Some)
}

fn persist_cli_run_started(request: &RunRequest) -> Result<(), String> {
    let record = RunSessionRecord::started(request);
    mirror_session_started(&request.config_path, &record)
}

struct CompletedRunContext<'a, 'sink> {
    path: &'a Path,
    config: &'a axon_config::AxonConfig,
    request: &'a RunRequest,
    elapsed_ms: u64,
    output: OutputFormat,
    progress: Option<&'a run_progress::RunProgressSink<'sink>>,
}

fn finish_successful_provider_run(
    context: &CompletedRunContext<'_, '_>,
    response: RunResponse,
) -> CliRunResult {
    match finish_completed_run(context.path, context.request, response) {
        Ok(completed) => {
            run_compaction::schedule_idle_compaction(
                context.path,
                context.config,
                &context.request.session_id,
                context.progress,
            );
            render_completed_run(
                context.request,
                completed,
                context.output,
                context.elapsed_ms,
            )
        }
        Err(error) => cli_error(format!("完成 Provider turn 持久化失败：{error}")),
    }
}

fn render_completed_run(
    request: &RunRequest,
    completed: CompletedRun,
    output: OutputFormat,
    provider_turn_elapsed_ms: u64,
) -> CliRunResult {
    let response = completed.response;
    let diagnostics = run_outcome_diagnostics::collect(
        &response,
        provider_turn_elapsed_ms,
        &request.persona_warnings,
    );
    match output {
        OutputFormat::Text => {
            let mut stdout = format!(
                "运行终态：{}\n{}",
                diagnostics.status,
                super::run_text_render::render_text_run_summary(request, &response),
            );
            // 文本输出同样承载降级告警：人格装配、校验与超时都在这里露出。
            for warning in &diagnostics.warnings {
                stdout.push_str(&format!("告警：{warning}\n"));
            }
            CliRunResult {
                exit_code: u8::from(diagnostics.status == "failed"),
                stdout,
                stderr: String::new(),
            }
        }
        OutputFormat::Json => {
            let event = RunJsonEvent::completed(CompletedRunWireEventsInput {
                request,
                response: &response,
                session_title: completed.session_title.as_deref(),
                memory_usefulness: completed.memory_usefulness.as_ref(),
                memory_autonomy: completed.memory_autonomy.as_ref(),
                skill_evolution: completed.skill_evolution.as_ref(),
                post_run_governance: completed.post_run_governance.as_ref(),
                recorded_at_ms: run_execution::wire_recorded_at_ms(),
                total_elapsed_ms: provider_turn_elapsed_ms,
            });
            let mut result = json_result(&event);
            if event.status == "failed" {
                result.exit_code = 1;
            }
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_terminal_cancels_all_running_subagents() {
        let first_path = AgentPath::parse("/root/first").unwrap();
        let second_path = AgentPath::parse("/root/second").unwrap();
        let first_token = RunCancelToken::default();
        let second_token = RunCancelToken::default();
        let cancel_tokens = Arc::new(Mutex::new(HashMap::from([
            (first_path, first_token.clone()),
            (second_path, second_token.clone()),
        ])));

        cancel_all_running_subagents(&cancel_tokens);

        assert!(first_token.is_cancelled());
        assert!(second_token.is_cancelled());
        assert!(cancel_tokens.lock().unwrap().is_empty());
    }
}
