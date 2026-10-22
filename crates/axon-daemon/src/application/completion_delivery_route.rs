//! Execution 完成通知的原始复合车道与投递目标解析。

use axon_domain::{AudienceContext, AudienceKind, ExecutionId};
use axon_session::{CompletionDeliveryTarget, RuntimeStore};

/// 飞书渠道标识。
const FEISHU_CHANNEL: &str = "feishu";
/// TUI 渠道标识。
const TUI_CHANNEL: &str = "tui";
/// Execution payload 中运行时上下文的字段名。
const TRANSPORT_CONTEXT_KEY: &str = "transportContext";
/// 运行时上下文中的原始来源渠道字段名。
const TRANSPORT_CHANNEL_KEY: &str = "channel";
/// 运行时上下文中的原始会话表面字段名。
const TRANSPORT_DESTINATION_KEY: &str = "destination";
/// 运行时上下文中的原始受众字段名。
const TRANSPORT_AUDIENCE_KEY: &str = "audience";

/// Execution 完成后可用的主 turn steer 路由。
pub(super) struct ExecutionCompletionRoute {
    pub(super) targets: Vec<CompletionDeliveryTarget>,
    /// 完成通知汇总所在的主 turn 车道；没有任何可用渠道时为空。
    ///
    /// 车道取自 Execution 自己的原始复合车道，与该渠道能否投递无关：HTTP/WS 等非
    /// 非通知渠道来源同样必须派发，否则汇总永不产出，连可用渠道的投递也会一起停在
    /// `pending` 直到隔天静默失效。
    pub(super) primary: Option<CompletionDeliveryTarget>,
    /// 创建 Task 时的受众信任域，禁止由投递渠道反推。
    pub(super) audience: AudienceContext,
}

/// 读取投递目标：当前 Workspace 的 TUI + Home 级外部渠道。
///
/// 返回空列表表示当前没有任何可投递渠道，调用方必须保留持久收件箱条目，
/// 而不是丢弃通知或提前确认 Result。
pub(in crate::application) fn resolve_completion_targets<S>(
    store: &S,
    workspace_id: &str,
    now_ms: u64,
    feishu_notification_chat_id: Option<&str>,
) -> Result<Vec<CompletionDeliveryTarget>, String>
where
    S: RuntimeStore,
{
    let mut targets = Vec::new();
    for presence in store
        .active_client_presences(workspace_id, now_ms)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|presence| presence.channel == TUI_CHANNEL)
    {
        push_delivery_target(&mut targets, presence.channel, presence.destination);
    }
    if let Some(chat_id) = feishu_notification_chat_id {
        push_delivery_target(&mut targets, FEISHU_CHANNEL.to_owned(), chat_id.to_owned());
    }
    Ok(targets)
}

/// 在在线渠道之外，补充 Execution payload 明确声明的持久投递目标。
pub(super) fn resolve_completion_targets_for_execution<S>(
    store: &S,
    workspace_id: &str,
    execution_id: &str,
    now_ms: u64,
    feishu_notification_chat_id: Option<&str>,
) -> Result<Vec<CompletionDeliveryTarget>, String>
where
    S: RuntimeStore,
{
    execution_completion_route(
        store,
        workspace_id,
        execution_id,
        now_ms,
        feishu_notification_chat_id,
    )
    .map(|route| route.targets)
}

/// 从 Execution payload 恢复原始复合车道与全部可投递渠道。
pub(super) fn execution_completion_route<S>(
    store: &S,
    workspace_id: &str,
    execution_id: &str,
    now_ms: u64,
    feishu_notification_chat_id: Option<&str>,
) -> Result<ExecutionCompletionRoute, String>
where
    S: RuntimeStore,
{
    let context = execution_transport_context(store, execution_id)?
        .ok_or_else(|| "Execution payload 缺少 transportContext".to_owned())?;
    let primary_route = transport_context_target(&context)?;
    let audience = transport_context_audience(&context)?;
    let mut targets =
        resolve_completion_targets(store, workspace_id, now_ms, feishu_notification_chat_id)?;
    if audience.kind() == AudienceKind::Group {
        targets.retain(|target| target == &primary_route);
    }
    // 主 turn 始终沿原车道汇总；渠道离线时可由已登记的备用渠道交付。
    let primary = (!targets.is_empty()).then_some(primary_route);
    Ok(ExecutionCompletionRoute {
        targets,
        primary,
        audience,
    })
}

/// 按主键读取 Execution 的运行时上下文。
fn execution_transport_context<S>(
    store: &S,
    execution_id: &str,
) -> Result<Option<serde_json::Value>, String>
where
    S: RuntimeStore,
{
    let Ok(execution_id) = ExecutionId::new(execution_id.to_owned()) else {
        return Ok(None);
    };
    let Some(execution) = store
        .get_execution(&execution_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let payload = serde_json::from_str::<serde_json::Value>(&execution.payload_json)
        .map_err(|error| format!("Execution payload 无效：{error}"))?;
    Ok(payload.get(TRANSPORT_CONTEXT_KEY).cloned())
}

/// 从运行时上下文恢复原始回复路由。
fn transport_context_target(
    context: &serde_json::Value,
) -> Result<CompletionDeliveryTarget, String> {
    let channel = context
        .get(TRANSPORT_CHANNEL_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Execution transportContext 缺少 channel".to_owned())?;
    let destination = context
        .get(TRANSPORT_DESTINATION_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Execution transportContext 缺少 destination".to_owned())?;
    Ok(CompletionDeliveryTarget {
        channel: channel.to_owned(),
        destination: destination.to_owned(),
    })
}

/// 从运行时上下文恢复原始受众信任域。
fn transport_context_audience(context: &serde_json::Value) -> Result<AudienceContext, String> {
    let audience = context
        .get(TRANSPORT_AUDIENCE_KEY)
        .ok_or_else(|| "Execution transportContext 缺少 audience".to_owned())?;
    serde_json::from_value(audience.clone())
        .map_err(|error| format!("Execution transportContext audience 无效：{error}"))
}

/// 追加一个投递目标；空目标与重复目标都不登记。
fn push_delivery_target(
    targets: &mut Vec<CompletionDeliveryTarget>,
    channel: String,
    destination: String,
) {
    if destination.trim().is_empty() {
        return;
    }
    let candidate = CompletionDeliveryTarget {
        channel,
        destination,
    };
    if !targets.contains(&candidate) {
        targets.push(candidate);
    }
}
