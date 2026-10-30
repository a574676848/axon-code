//! Execution 完成通知的持久投递目标解析。
//!
//! 产品定义：完成通知只回到创建该 Task 时的会话表面，不再向所有在线客户端广播。
//! 显式目标来自 Execution payload 的 `transportContext`，那是创建时就已经确定的
//! 事实，不需要在完成时刻重新推断「谁可能在线」。

use axon_domain::{AudienceContext, ExecutionId};
use axon_session::{CompletionDeliveryTarget, RuntimeStore};

/// Execution payload 中运行时上下文的字段名。
const TRANSPORT_CONTEXT_KEY: &str = "transportContext";
/// 运行时上下文中的原始来源渠道字段名。
const TRANSPORT_CHANNEL_KEY: &str = "channel";
/// 运行时上下文中的原始会话表面字段名。
const TRANSPORT_DESTINATION_KEY: &str = "destination";
/// 运行时上下文中的原始受众字段名。
const TRANSPORT_AUDIENCE_KEY: &str = "audience";

/// Execution 完成后可用的投递路由。
pub(super) struct ExecutionCompletionRoute {
    /// 唯一投递目标：创建该 Task 时的会话表面。
    pub(super) target: CompletionDeliveryTarget,
    /// 创建 Task 时的受众信任域，禁止由投递渠道反推。
    pub(super) audience: AudienceContext,
}

/// 从 Execution payload 恢复原始复合车道与受众。
pub(super) fn execution_completion_route<S>(
    store: &S,
    execution_id: &str,
) -> Result<ExecutionCompletionRoute, String>
where
    S: RuntimeStore,
{
    let context = execution_transport_context(store, execution_id)?
        .ok_or_else(|| "Execution payload 缺少 transportContext".to_owned())?;
    Ok(ExecutionCompletionRoute {
        target: transport_context_target(&context)?,
        audience: transport_context_audience(&context)?,
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
