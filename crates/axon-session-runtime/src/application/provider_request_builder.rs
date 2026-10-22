//! Provider 请求的统一构造边界。

use axon_domain::QuotedMessageContext;
use axon_session::SessionActionRecord;
use serde::Deserialize;

use crate::application::jarvis_runtime_profile::JarvisRuntimeProfile;
use crate::model::SessionExecutionContext;

/// Provider runner 的规范化输入。
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderTurnRequest {
    /// Session Runtime 的稳定 session ID。
    pub session_id: String,
    /// 持久 Execution 类型，用于来源治理与后台回合隔离。
    pub action_kind: String,
    /// 用户 prompt。
    pub prompt: String,
    /// 被回复消息的可见文本上下文。
    pub quoted_message: Option<QuotedMessageContext>,
    /// Session Runtime 决定的工作目录。
    pub cwd: String,
    /// Session Runtime 决定的持久 run session ID。
    pub run_session_id: String,
    /// 应用运行时选择的统一人格 profile。
    pub runtime_profile: JarvisRuntimeProfile,
    /// 由 transport 写入、由事件 journal 原样保留的上下文。
    pub transport_context: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderTurnPayload {
    prompt: String,
    #[serde(default)]
    quoted_message: Option<QuotedMessageContext>,
    #[serde(default)]
    transport_context: Option<serde_json::Value>,
}

/// 从持久 Execution 与权威运行上下文构造 Provider 请求。
///
/// payload 只承载用户 prompt 与 transport 数据，执行身份始终来自 `context`。
pub fn build_provider_turn_request(
    action: &SessionActionRecord,
    context: &SessionExecutionContext,
    runtime_profile: JarvisRuntimeProfile,
) -> Result<ProviderTurnRequest, String> {
    let payload: ProviderTurnPayload = serde_json::from_str(&action.payload_json)
        .map_err(|error| format!("Action payload 解析失败：{error}"))?;
    Ok(ProviderTurnRequest {
        session_id: action.session_id.to_string(),
        action_kind: action.kind.clone(),
        prompt: payload.prompt,
        quoted_message: payload.quoted_message,
        cwd: context.cwd.clone(),
        run_session_id: context.run_session_id.clone(),
        runtime_profile,
        transport_context: payload.transport_context,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{DaemonSessionId, SessionActionId, SessionActionStatus};

    fn action(payload_json: &str) -> SessionActionRecord {
        SessionActionRecord {
            action_id: SessionActionId::new("action-builder").expect("有效 action"),
            goal_id: None,
            session_id: DaemonSessionId::new("session-builder").expect("有效 session"),
            kind: "user_input".to_owned(),
            priority: 0,
            status: SessionActionStatus::Queued,
            dedupe_key: None,
            payload_json: payload_json.to_owned(),
            goal_revision: None,
            context_version: None,
            claimed_by: None,
            claimed_generation: None,
            lease_expires_at_ms: None,
            attempt: 0,
        }
    }

    fn context() -> SessionExecutionContext {
        SessionExecutionContext {
            run_session_id: "run-builder".to_owned(),
            cwd: "workspace".to_owned(),
            context_version: 1,
        }
    }

    #[test]
    fn builder_preserves_prompt_and_transport_shape() {
        let request = build_provider_turn_request(
            &action(
                r#"{"prompt":"问题","quotedMessage":{"messageId":"parent","visibleText":"引用"},"transportContext":{"kind":"feishu","chatId":"chat"}}"#,
            ),
            &context(),
            JarvisRuntimeProfile::Jarvis,
        )
        .expect("请求应可构造");

        assert_eq!(request.prompt, "问题");
        assert_eq!(request.session_id, "session-builder");
        assert_eq!(request.run_session_id, "run-builder");
        assert_eq!(request.cwd, "workspace");
        assert_eq!(request.runtime_profile, JarvisRuntimeProfile::Jarvis);
        assert_eq!(
            request
                .transport_context
                .as_ref()
                .and_then(|value| value.get("kind"))
                .and_then(serde_json::Value::as_str),
            Some("feishu")
        );
        assert_eq!(
            request
                .quoted_message
                .as_ref()
                .map(|value| value.visible_text.as_str()),
            Some("引用")
        );
    }

    #[test]
    fn builder_rejects_execution_identity_in_payload() {
        let error = build_provider_turn_request(
            &action(r#"{"prompt":"问题","cwd":"错误目录","runSessionId":"错误会话","runtimeProfile":"other"}"#),
            &context(),
            JarvisRuntimeProfile::Jarvis,
        )
        .expect_err("payload 不得覆盖权威执行上下文");

        assert!(error.contains("unknown field"));
    }
}
