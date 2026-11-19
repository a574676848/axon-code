//! 一轮对话的开始：起始持久化与 stop 控制通道。
//!
//! 两个 turn 入口共用这一处。共用之外只剩装载端的登记动作（子代理把自己登记进父轮名册、
//! 把会话状态推进到运行中），那不是「一轮怎么开始」，而是「谁开始了一轮」。

use super::{
    RunRequest, RunSessionRecord, RunStopControl, mirror_session_started, run_turn_spec::TurnSpec,
};

/// 落一轮的起始态：记录构造与投影只有这一份。
///
/// 返回记录本身：子代理要用同一条记录补它自己的标题投影，而不是再造一条。
pub(crate) fn persist_turn_started(request: &RunRequest) -> Result<RunSessionRecord, String> {
    let record = RunSessionRecord::started(request);
    mirror_session_started(&request.config_path, &record)?;
    // 会话行上的轮次标识就是「当前这一轮是谁」：主会话用它记入站事实，
    // 目标执行记执行标识（由 worker 在领到动作时绑定），两者同一列语义。
    if !request.interaction_id.is_empty() {
        bind_turn_execution(
            &record.session_id,
            &request.interaction_id,
            &request.config_path,
        );
    }
    Ok(record)
}

fn bind_turn_execution(session_id: &str, execution_id: &str, config_path: &std::path::Path) {
    let Ok(session_id) = axon_domain::DaemonSessionId::new(session_id.to_owned()) else {
        return;
    };
    let Ok(store) = super::run_turn_metrics::shared_store(config_path) else {
        return;
    };
    use axon_session::RuntimeStore as _;
    if let Err(error) =
        store.bind_session_execution(&session_id, execution_id, super::run_execution::now_ms())
    {
        axon_logger::warning(&format!(
            "turn execution binding skipped: session_id={}, error={error}",
            session_id.as_str()
        ));
    }
}

/// 为运行侧自己派发的轮次补一条输入事实。
///
/// 内容就是本轮交给模型的完整提示（子代理是任务提示，目标执行是目标要干的事的全文），
/// 不是摘要：会话视图里用户看到的第一条消息必须是真实输入。
pub(crate) fn record_turn_input(spec: &TurnSpec, request: &RunRequest) {
    let Some((channel, client)) = spec.input.generated() else {
        return;
    };
    if let Err(error) = append_turn_input(channel, client, request) {
        axon_logger::warning(&format!(
            "turn input skipped: session_id={}, error={error}",
            request.session_id
        ));
    }
}

fn append_turn_input(
    channel: &'static str,
    client: Option<&'static str>,
    request: &RunRequest,
) -> Result<(), String> {
    use axon_session::{InteractionStore as _, RuntimeStore as _};
    let store = super::run_turn_metrics::shared_store(&request.config_path)?;
    let session_id = axon_domain::DaemonSessionId::new(request.session_id.clone())
        .map_err(|error| format!("会话标识非法：{error:?}"))?;
    let session = store
        .get_session(&session_id)
        .map_err(|error| format!("会话不可读：{error:?}"))?;
    let workspace_id = session
        .workspace_id
        .clone()
        .ok_or_else(|| "会话缺少工作区绑定".to_owned())?;
    // 来源键含会话与调用：同一次派发重放命中同一行，不同轮次各自成行。
    let source = with_client(
        axon_domain::InteractionSource::new(
            channel,
            format!(
                "{}{}:{}",
                axon_domain::AGENT_DISPATCH_SOURCE_PREFIX,
                request.session_id,
                request.invocation_id
            ),
        )
        .map_err(|error| error.to_string())?,
        client,
    )?;
    let interaction = axon_domain::InteractionRecord::new(
        session_id,
        axon_session::new_interaction_id(),
        source,
        axon_domain::ScopeRef::workspace(workspace_id).map_err(|error| error.to_string())?,
        axon_domain::AudienceContext::new(
            axon_domain::AudienceKind::Private,
            request.goal_disclosure_policy,
        ),
        // 回执回到发起方：子代理回父会话，目标执行回到它自己的看板入口。
        axon_domain::ReplyRoute::new(
            channel,
            request
                .parent_session_id
                .clone()
                .unwrap_or_else(|| request.session_id.clone()),
        )
        .map_err(|error| error.to_string())?,
        request.prompt.clone(),
        super::run_execution::now_ms(),
    )
    .map_err(|error| error.to_string())?;
    store
        .append(&interaction)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn with_client(
    source: axon_domain::InteractionSource,
    client: Option<&'static str>,
) -> Result<axon_domain::InteractionSource, String> {
    match client {
        Some(client) => source
            .with_client(client)
            .map_err(|error| error.to_string()),
        None => Ok(source),
    }
}

/// 起一条 run-control 通道：让 `axon stop` 能停住这一轮。
///
/// 没有取消令牌就无从建立通道。要不要通道、失败是否致命，由调用端按自己的来源决定，
/// 这里只负责“有令牌就建”。
pub(crate) fn start_turn_stop_control(
    request: &RunRequest,
) -> Result<Option<RunStopControl>, String> {
    let Some(cancel_token) = request.cancel_token.clone() else {
        return Ok(None);
    };
    RunStopControl::start(
        &request.config_path,
        &request.session_id,
        &request.run_session_id,
        cancel_token,
    )
    .map(Some)
}
