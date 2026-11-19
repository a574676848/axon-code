use super::{
    CliRunResult, CompletedRunWireEventsInput, ConfigLoader, OutputFormat, RunAgentIdentity,
    RunCancelToken, RunCommand, RunJsonEvent, RunRequest, RunRequestBuildOptions,
    RunRequestSession, RunStartupSnapshot, cli_config_error, cli_error, json_result,
    load_run_env_file, new_run_session_id, prepare_run_session, resolved_config_path,
    run_execution, run_outcome_diagnostics, run_progress,
    run_request_payload::RunWorkspaceSnapshotPrewarm,
    run_steer,
    run_turn_execution::{TurnTeardownScope, execute_turn},
    run_turn_spec::{TurnInput, TurnSpec},
    run_turn_start::{persist_turn_started, record_turn_input, start_turn_stop_control},
    run_turn_terminal::{CompletedTurn, FailedTurn, complete_turn, fail_turn},
};
use axon_client_identity::ClientChannel;
use axon_domain::QuotedMessageContext;
use axon_session_runtime::{
    SubagentSessionSinkFactory, TurnOrigin, TurnPerformanceRecorder, TurnPerformanceStage,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
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
    /// 本轮来源：装载与收口口径由它决定，入口不得再自行假设「一律主 turn」。
    pub(crate) turn_origin: TurnOrigin,
    /// 本轮的输入事实由谁落：入站轮已由前端提交，派发轮由运行侧补写。
    pub(crate) turn_input: TurnInput,
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
            turn_origin: TurnOrigin::Primary,
            turn_input: TurnInput::Submitted,
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
            match super::interaction_adapter::conversation_session_run_session_id(
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
            .map(|session| session.facts.session_id.clone()),
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
    let cancel_token = run_cancel_token(&options);
    let progress = options.progress.map(|emit| {
        let sink = run_progress::RunProgressSink::with_session_id(emit, session_id.clone());
        // 超预算的工具结果全文落在运行态数据根下，事件里只带路径。
        match crate::runtime_paths::runtime_axon_data_path_static(&path) {
            Ok(root) => sink.with_tool_result_archive_root(root),
            Err(_) => sink,
        }
    });
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
        .and_then(|session| session.facts.cwd.as_deref())
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
    let spec = TurnSpec::for_origin(options.turn_origin, options.turn_input);
    // 派发轮先落输入事实：会话视图的第一条消息因此是真实输入而不是空。
    record_turn_input(&spec, &request);
    if let Err(error) = persist_turn_started(&request) {
        return cli_error(format!("Provider turn 起始持久化失败：{error}"));
    }
    if let Some(performance) = &request.performance {
        performance.record(TurnPerformanceStage::ThreadStartMirrorReady);
        performance.record(TurnPerformanceStage::RunStartedPersisted);
    }
    // 主 turn 只有在调用方要求外部停止源时才暴露 run-control 通道；失败即中止本轮。
    let _stop_control = if options.external_stop_control {
        match start_turn_stop_control(&request) {
            Ok(control) => control,
            Err(error) => return cli_error(error),
        }
    } else {
        None
    };
    let teardown = TurnTeardownScope::primary(&request);
    let execution = execute_turn(&request, progress.as_ref(), &teardown);
    let teardown_started_ms = run_execution::now_ms();
    let elapsed_ms = run_execution::now_ms().saturating_sub(execution.started_at_ms);
    let run_result = match execution.provider {
        Ok(response) => render_completed_turn(
            &request,
            complete_turn(&spec, &request, response),
            output,
            elapsed_ms,
        ),
        Err(error) => {
            let partial_response = progress
                .as_ref()
                .map_or_else(String::new, |sink| sink.partial_assistant_content());
            let failed = fail_turn(
                &path,
                &loaded.config,
                &spec,
                &request,
                error,
                partial_response,
            );
            render_failed_turn(&request, &failed, output)
        }
    };
    axon_logger::debug(&format!(
        "run 收尾持久化完成: session={}, teardown_ms={}",
        request.session_id,
        run_execution::now_ms().saturating_sub(teardown_started_ms)
    ));
    run_result
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

fn render_completed_turn(
    request: &RunRequest,
    completed: CompletedTurn,
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
            let mut event = RunJsonEvent::completed(CompletedRunWireEventsInput {
                request,
                response: &response,
                session_title: completed.session_title.as_deref(),
                memory_usefulness: completed.memory_usefulness.as_ref(),
                memory_autonomy: completed.memory_autonomy.as_ref(),
                post_run_governance: completed.post_run_governance.as_ref(),
                recorded_at_ms: run_execution::wire_recorded_at_ms(),
                total_elapsed_ms: provider_turn_elapsed_ms,
            });
            event.memory_reflection_notice = completed.memory_reflection_notice.clone();
            let mut result = json_result(&event);
            if event.status == "failed" {
                result.exit_code = 1;
            }
            result
        }
    }
}

fn render_failed_turn(
    request: &RunRequest,
    failed: &FailedTurn,
    output: OutputFormat,
) -> CliRunResult {
    let report = RunJsonEvent::failed(
        request,
        &failed.error,
        &failed.failure,
        failed.memory_usefulness.as_ref(),
        failed.memory_autonomy.as_ref(),
    );
    match output {
        OutputFormat::Text => CliRunResult {
            exit_code: 1,
            stdout: super::run_text_render::render_text_run_failure(request, &failed.error),
            stderr: String::new(),
        },
        OutputFormat::Json => {
            let mut result = json_result(&report);
            result.exit_code = 1;
            result
        }
    }
}
