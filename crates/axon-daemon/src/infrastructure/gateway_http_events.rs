//! Gateway 入站事件的归一化与拒绝审计。
//!
//! 这里只处理「报文到事件」这一段：路由形态由 axum 决定，本模块负责 provider
//! 名校验、解析脚本的失败映射与信封装配，失败时直接给出既定响应。

use super::gateway_http_response::json_error;
use crate::application::ExternalEventEnvelope;
use crate::infrastructure::event_scripts::{
    EventProviderRegistry, EventScriptError, EventScriptEvent,
};
use axum::response::Response;
use serde::Deserialize;

/// provider 事件默认有效期。
const PROVIDER_EVENT_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

/// 入站事件路由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GatewayRoute {
    /// 统一入口：provider 由报文 `sourceChannel` 指定。
    Generic,
    /// provider 统一信封：发件方已完成归一化，不调用解析脚本。
    Envelope(String),
    /// provider 原生直通：报文交给 `parser.py` 归一化。
    Raw(String),
    /// 交互记录与处理结果查询入口。
    Interaction(String),
}

impl GatewayRoute {
    /// 按 provider 名校验结果构造 provider 路由；非法名返回 None（对外即 404）。
    pub(super) fn provider(provider: &str, raw: bool) -> Option<Self> {
        EventProviderRegistry::validate_provider_name(provider)
            .ok()
            .map(|()| {
                if raw {
                    Self::Raw(provider.to_owned())
                } else {
                    Self::Envelope(provider.to_owned())
                }
            })
    }

    /// 交互查询路由；空 ID 一律返回 None。
    pub(super) fn interaction(id: &str) -> Option<Self> {
        let id = id.trim();
        (!id.is_empty()).then(|| Self::Interaction(id.to_owned()))
    }
}

/// 按路由把入站报文归一化为外部事件；失败时返回已经确定的响应。
pub(super) fn normalized_event(
    route: &GatewayRoute,
    body: &[u8],
    providers: &EventProviderRegistry,
) -> Result<ExternalEventEnvelope, Response> {
    match route {
        GatewayRoute::Generic => {
            let input = serde_json::from_slice::<HttpEventRequest>(body)
                .map_err(|_| json_error(400, "invalid_json"))?;
            input
                .into_event()
                .map_err(|()| json_error(400, "invalid_event"))
        }
        GatewayRoute::Envelope(provider) | GatewayRoute::Raw(provider) => {
            if !providers.is_registered(provider) {
                audit_event_rejection(provider, "provider 未注册");
                return Err(json_error(404, "provider_not_configured"));
            }
            let received_at_ms = system_now_ms();
            let parsed = match route {
                GatewayRoute::Raw(_) => {
                    parse_provider_payload(provider, body, providers, received_at_ms)?
                }
                _ => serde_json::from_slice::<EventScriptEvent>(body)
                    .map_err(|_| json_error(400, "invalid_event"))?,
            };
            let event = EventScriptEvent::validate(provider, parsed).map_err(|error| {
                audit_event_rejection(provider, &error.to_string());
                json_error(400, "invalid_event")
            })?;
            Ok(provider_envelope(provider, event, received_at_ms))
        }
        GatewayRoute::Interaction(_) => Err(json_error(400, "invalid_event")),
    }
}

fn parse_provider_payload(
    provider: &str,
    body: &[u8],
    providers: &EventProviderRegistry,
    received_at_ms: u64,
) -> Result<EventScriptEvent, Response> {
    let payload = serde_json::from_slice::<serde_json::Value>(body)
        .map_err(|_| json_error(400, "invalid_json"))?;
    providers
        .run_parser(provider, &payload, received_at_ms)
        .map_err(|error| {
            audit_event_rejection(provider, &error.to_string());
            let (status, code) = script_failure_response(&error);
            json_error(status, code)
        })
}

fn script_failure_response(error: &EventScriptError) -> (u16, &'static str) {
    match error {
        EventScriptError::InvalidProvider(_)
        | EventScriptError::UnknownProvider(_)
        | EventScriptError::MissingParser(_) => (404, "provider_not_configured"),
        EventScriptError::Timeout => (504, "event_script_timeout"),
        EventScriptError::Spawn(_)
        | EventScriptError::Failed { .. }
        | EventScriptError::InvalidJson(_)
        | EventScriptError::InvalidOutput(_) => (502, "event_script_failed"),
    }
}

fn provider_envelope(
    provider: &str,
    event: EventScriptEvent,
    received_at_ms: u64,
) -> ExternalEventEnvelope {
    let correlation_key = event
        .correlation_key
        .clone()
        .unwrap_or_else(|| event.event_id.clone());
    ExternalEventEnvelope {
        source_channel: provider.to_owned(),
        event_id: event.event_id,
        workspace_id: String::new(),
        observed_at_ms: received_at_ms,
        expires_at_ms: received_at_ms.saturating_add(PROVIDER_EVENT_TTL_MS),
        content: event.content,
        title: Some(event.title),
        correlation_key: Some(correlation_key.clone()),
        reply_channel: provider.to_owned(),
        reply_destination: correlation_key,
        attachments: Vec::new(),
    }
}

/// 审计被丢弃的外部事件：fail-closed，不进入耐久存储。
fn audit_event_rejection(provider: &str, reason: &str) {
    eprintln!("[gateway-events] provider={provider} 事件被丢弃：{reason}");
}

fn system_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HttpEventRequest {
    source_channel: String,
    event_id: String,
    /// 事件所属 Workspace。
    ///
    /// 缺省（或仅有空白）时由服务端按运行态解析默认 Workspace；显式给出时
    /// 必须命中 Workspace 注册表，否则整条事件被拒绝。外部 webhook 发送方
    /// 无从得知工作区标识，只有自带工作区上下文的客户端（例如桌面工作台）才会填写。
    #[serde(default)]
    workspace_id: Option<String>,
    observed_at_ms: u64,
    expires_at_ms: u64,
    content: String,
    reply_channel: String,
    reply_destination: String,
    /// 本次提交携带的本地附件绝对路径。
    #[serde(default)]
    local_files: Vec<String>,
}

impl HttpEventRequest {
    fn into_event(self) -> Result<ExternalEventEnvelope, ()> {
        Ok(ExternalEventEnvelope {
            source_channel: self.source_channel,
            event_id: self.event_id,
            workspace_id: self.workspace_id.unwrap_or_default().trim().to_owned(),
            observed_at_ms: self.observed_at_ms,
            expires_at_ms: self.expires_at_ms,
            content: self.content,
            title: None,
            correlation_key: None,
            reply_channel: self.reply_channel,
            reply_destination: self.reply_destination,
            attachments: self.local_files,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_names_are_validated_before_routing() {
        assert_eq!(
            GatewayRoute::provider("jira", false),
            Some(GatewayRoute::Envelope("jira".to_owned()))
        );
        assert_eq!(
            GatewayRoute::provider("jira", true),
            Some(GatewayRoute::Raw("jira".to_owned()))
        );
        for invalid in ["", "JIRA", "..", "jira/extra", "jira.raw"] {
            assert_eq!(
                GatewayRoute::provider(invalid, false),
                None,
                "{invalid} 必须被拒绝"
            );
        }
    }

    #[test]
    fn interaction_ids_reject_empty_values() {
        assert_eq!(
            GatewayRoute::interaction(" int-1 "),
            Some(GatewayRoute::Interaction("int-1".to_owned()))
        );
        assert_eq!(GatewayRoute::interaction("  "), None);
    }

    #[test]
    fn script_failures_map_to_distinct_fail_closed_statuses() {
        assert_eq!(
            script_failure_response(&EventScriptError::MissingParser("jira".to_owned())),
            (404, "provider_not_configured")
        );
        assert_eq!(
            script_failure_response(&EventScriptError::Timeout),
            (504, "event_script_timeout")
        );
        assert_eq!(
            script_failure_response(&EventScriptError::InvalidJson("坏输出".to_owned())),
            (502, "event_script_failed")
        );
    }

    #[test]
    fn event_wire_allows_server_side_workspace_resolution() {
        let request = serde_json::from_str::<HttpEventRequest>(
            r#"{"sourceChannel":"http","eventId":"event-1","observedAtMs":100,"expiresAtMs":200,"content":"hello","replyChannel":"http","replyDestination":"client-1"}"#,
        )
        .expect("缺省 workspaceId 应可解析");
        assert!(
            request
                .into_event()
                .expect("事件映射")
                .workspace_id
                .is_empty()
        );
    }

    #[test]
    fn event_wire_carries_explicit_workspace_id_and_rejects_unknown_fields() {
        let request = serde_json::from_str::<HttpEventRequest>(
            r#"{"sourceChannel":"http","eventId":"event-1","workspaceId":"workspace-a","observedAtMs":100,"expiresAtMs":200,"content":"hello","replyChannel":"http","replyDestination":"client-1"}"#,
        )
        .expect("显式 workspaceId 必须可解析");
        assert_eq!(
            request.into_event().expect("事件映射").workspace_id,
            "workspace-a"
        );
        assert!(
            serde_json::from_str::<HttpEventRequest>(
                r#"{"sourceChannel":"http","eventId":"event-1","observedAtMs":100,"expiresAtMs":200,"content":"hello","replyChannel":"http","replyDestination":"client-1","unknownField":true}"#,
            )
            .is_err(),
            "未知字段必须继续被拒绝"
        );
    }

    #[test]
    fn provider_envelope_carries_namespace_key_and_ttl() {
        let event = provider_envelope(
            "jira",
            EventScriptEvent {
                event_id: "jira:AXON-1".to_owned(),
                correlation_key: None,
                title: "工单已指派".to_owned(),
                content: "正文".to_owned(),
            },
            1_000,
        );
        assert_eq!(event.source_channel, "jira");
        assert_eq!(event.reply_channel, "jira");
        assert_eq!(event.reply_destination, "jira:AXON-1");
        assert_eq!(event.correlation_key.as_deref(), Some("jira:AXON-1"));
        assert_eq!(event.title.as_deref(), Some("工单已指派"));
        assert_eq!(event.observed_at_ms, 1_000);
        assert_eq!(event.expires_at_ms, 1_000 + PROVIDER_EVENT_TTL_MS);
    }
}
