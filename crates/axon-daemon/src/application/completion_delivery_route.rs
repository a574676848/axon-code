//! Execution 完成通知的持久投递目标解析。
//!
//! 产品定义：完成通知只回到创建该 Task 时的会话表面，不再向所有在线客户端广播。
//! 显式目标来自 Execution payload 的 `transportContext`，那是创建时就已经确定的
//! 事实，不需要在完成时刻重新推断「谁可能在线」。

use axon_domain::{AudienceContext, DaemonSessionId, ExecutionId};
use axon_session::{CompletionDeliveryTarget, RuntimeStore};

/// Execution payload 中运行时上下文的字段名。
const TRANSPORT_CONTEXT_KEY: &str = "transportContext";
/// 运行时上下文中的原始来源渠道字段名。
const TRANSPORT_CHANNEL_KEY: &str = "channel";
/// 运行时上下文中的原始会话表面字段名。
const TRANSPORT_DESTINATION_KEY: &str = "destination";
/// 运行时上下文中的原始受众字段名。
const TRANSPORT_AUDIENCE_KEY: &str = "audience";
/// 运行时上下文中的会话标识字段名。
const TRANSPORT_SESSION_ID_KEY: &str = "primarySessionId";

/// Execution 完成后可用的投递路由。
pub(super) struct ExecutionCompletionRoute {
    /// 唯一投递目标：创建该 Task 时的会话表面。
    pub(super) target: CompletionDeliveryTarget,
    /// 创建 Task 时的受众信任域，禁止由投递渠道反推。
    pub(super) audience: AudienceContext,
    /// 创建该 Task 时的会话标识：完成回流必须落回同一条会话。
    pub(super) session_id: DaemonSessionId,
}

/// 从 Execution 记录与 payload 恢复原始会话与受众。
pub(super) fn execution_completion_route<S>(
    store: &S,
    execution_id: &str,
) -> Result<ExecutionCompletionRoute, String>
where
    S: RuntimeStore,
{
    let Ok(parsed_id) = ExecutionId::new(execution_id.to_owned()) else {
        return Err("Execution ID 无效".to_owned());
    };
    let execution = store
        .get_execution(&parsed_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Execution 不存在：{execution_id}"))?;
    let payload = serde_json::from_str::<serde_json::Value>(&execution.payload_json)
        .map_err(|error| format!("Execution payload 无效：{error}"))?;
    let context = payload
        .get(TRANSPORT_CONTEXT_KEY)
        .ok_or_else(|| "Execution payload 缺少 transportContext".to_owned())?;
    let session_id = completion_session_id(execution.session_id.clone(), context)?;
    Ok(ExecutionCompletionRoute {
        target: transport_context_target(context)?,
        audience: transport_context_audience(context)?,
        session_id,
    })
}

/// 完成回流落回**创建这一轮的主会话**。
///
/// 执行行上的 session 是执行自己那条（目标会话），把它当投递目标会让完成通知在目标
/// 会话里自己唤醒自己，创建者永远不知道结果。旧载荷没有 `primarySessionId` 时才退回
/// 执行行绑定。
fn completion_session_id(
    execution_session_id: Option<DaemonSessionId>,
    context: &serde_json::Value,
) -> Result<DaemonSessionId, String> {
    match transport_context_session_id(context) {
        Ok(session_id) => Ok(session_id),
        Err(error) => execution_session_id.ok_or(error),
    }
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

/// 从运行时上下文恢复会话标识。
fn transport_context_session_id(context: &serde_json::Value) -> Result<DaemonSessionId, String> {
    let session_id = context
        .get(TRANSPORT_SESSION_ID_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Execution transportContext 缺少 primarySessionId".to_owned())?;
    DaemonSessionId::new(session_id.to_owned())
        .map_err(|error| format!("Execution transportContext primarySessionId 无效：{error}"))
}

/// 从运行时上下文恢复原始受众信任域。
fn transport_context_audience(context: &serde_json::Value) -> Result<AudienceContext, String> {
    let audience = context
        .get(TRANSPORT_AUDIENCE_KEY)
        .ok_or_else(|| "Execution transportContext 缺少 audience".to_owned())?;
    serde_json::from_value(audience.clone())
        .map_err(|error| format!("Execution transportContext audience 无效：{error}"))
}

#[cfg(test)]
mod tests {
    use super::completion_session_id;
    use axon_domain::DaemonSessionId;

    fn session(value: &str) -> DaemonSessionId {
        DaemonSessionId::new(value.to_owned()).expect("会话标识")
    }

    /// 创建者主会话优先于执行行绑定：否则完成通知只会唤醒目标会话自己。
    #[test]
    fn completion_returns_to_the_creating_primary_session() {
        let context = serde_json::json!({ "primarySessionId": "01PRIMARYSESSION" });

        let routed = completion_session_id(Some(session("01GOALSESSION")), &context)
            .expect("应解析出投递会话");

        assert_eq!(routed.as_str(), "01PRIMARYSESSION");
    }

    /// 旧载荷没有 `primarySessionId` 时退回执行行绑定，不丢投递。
    #[test]
    fn completion_falls_back_to_the_execution_binding() {
        let context = serde_json::json!({});

        let routed = completion_session_id(Some(session("01GOALSESSION")), &context)
            .expect("应回落到执行行绑定");

        assert_eq!(routed.as_str(), "01GOALSESSION");
    }

    /// 两者都缺时必须报错：静默丢一次完成通知等于父轮永远等不到结果。
    #[test]
    fn completion_without_any_session_is_an_error() {
        assert!(completion_session_id(None, &serde_json::json!({})).is_err());
    }
}
