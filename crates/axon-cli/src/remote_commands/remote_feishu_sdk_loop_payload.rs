use std::path::PathBuf;

use axon_domain::{AxonError, AxonResult, SessionId};
use axon_remote_feishu::{
    RemoteFeishuGatewayEventSourceWriterRuntime, RemoteFeishuGatewayRawReactionAppendInput,
    RemoteFeishuGatewaySdkLoopAdapterRuntime, RemoteFeishuGatewaySdkLoopEvent,
    RemoteFeishuGatewaySdkLoopRunnerInput, RemoteFeishuGatewaySdkPayloadMapperRuntime,
    RemoteFeishuGatewaySdkPayloadTextContext, RemoteFeishuHttpRequestPlan,
    RemoteFeishuHttpResponse, RemoteFeishuOapiExecutorRuntime, RemoteFeishuOapiWsIncomingPayload,
};
use serde_json::Value;

use super::{REMOTE_FEISHU_SDK_LOOP_CLIENT_ID, RemoteFeishuSdkLoopPayloadLine, now_ms};

const SDK_LOOP_KIND_MESSAGE: &str = "message";
const SDK_LOOP_KIND_CARD_ACTION: &str = "cardAction";
const SDK_LOOP_KIND_CARD_ACTION_KEBAB: &str = "card-action";
const SDK_LOOP_KIND_REACTION: &str = "reaction";
const SDK_LOOP_KIND_IGNORED: &str = "ignored";
const SDK_LOOP_REACTION_TYPE_KEY: &str = "reaction_type";
const SDK_LOOP_EVENT_TYPE_KEY: &str = "event_type";
const SDK_LOOP_HEADER_KEY: &str = "header";
const SDK_LOOP_OPERATOR_TYPE_KEY: &str = "operator_type";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RealSdkPayloadAppendReport {
    pub input_payloads: usize,
    pub mapped_events: usize,
    pub appended_events: usize,
    pub received_acked_events: usize,
    pub received_ack_failed_events: usize,
}

#[derive(Debug, Clone)]
pub(super) struct RealSdkPayloadAppendInput {
    pub app_id: String,
    pub session_id: u64,
    pub event_source_file: PathBuf,
    pub text_context: Option<RemoteFeishuGatewaySdkPayloadTextContext>,
    pub tenant_access_token: String,
    pub api_base_url: Option<String>,
}

#[cfg(test)]
pub(super) fn append_real_sdk_payload(
    input: RealSdkPayloadAppendInput,
    payload: &RemoteFeishuOapiWsIncomingPayload,
) -> AxonResult<RealSdkPayloadAppendReport> {
    append_real_sdk_payload_with_http(
        input,
        payload,
        RemoteFeishuOapiExecutorRuntime::execute_json,
    )
}

pub(super) fn append_real_sdk_payload_with_steer<F>(
    input: RealSdkPayloadAppendInput,
    payload: &RemoteFeishuOapiWsIncomingPayload,
    mut try_steer: F,
) -> AxonResult<RealSdkPayloadAppendReport>
where
    F: FnMut(&RemoteFeishuGatewaySdkLoopEvent) -> AxonResult<bool>,
{
    append_real_sdk_payload_with_handlers(
        input,
        payload,
        RemoteFeishuOapiExecutorRuntime::execute_json,
        &mut try_steer,
    )
}

#[cfg(test)]
pub(super) fn append_real_sdk_payload_with_http<F>(
    input: RealSdkPayloadAppendInput,
    payload: &RemoteFeishuOapiWsIncomingPayload,
    mut http: F,
) -> AxonResult<RealSdkPayloadAppendReport>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
{
    let mut never_steer = |_event: &RemoteFeishuGatewaySdkLoopEvent| Ok(false);
    append_real_sdk_payload_with_handlers(input, payload, &mut http, &mut never_steer)
}

fn append_real_sdk_payload_with_handlers<F, S>(
    input: RealSdkPayloadAppendInput,
    payload: &RemoteFeishuOapiWsIncomingPayload,
    _http: F,
    mut try_steer: S,
) -> AxonResult<RealSdkPayloadAppendReport>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    S: FnMut(&RemoteFeishuGatewaySdkLoopEvent) -> AxonResult<bool>,
{
    let _ = (
        &input.app_id,
        &input.tenant_access_token,
        &input.api_base_url,
    );
    let value = serde_json::from_slice::<Value>(&payload.payload)
        .map_err(|_| AxonError::runtime_failure("Feishu SDK loop payload JSON 无效"))?;
    let line = RemoteFeishuSdkLoopPayloadLine {
        kind: sdk_loop_kind(&value)?,
        payload: value,
    };
    if line.kind == SDK_LOOP_KIND_IGNORED {
        return Ok(RealSdkPayloadAppendReport {
            input_payloads: 1,
            mapped_events: 0,
            appended_events: 0,
            received_acked_events: 0,
            received_ack_failed_events: 0,
        });
    }
    if line.kind == SDK_LOOP_KIND_REACTION {
        if reaction_operator_is_app_or_bot(&line.payload) {
            return Ok(RealSdkPayloadAppendReport {
                input_payloads: 1,
                mapped_events: 0,
                appended_events: 0,
                received_acked_events: 0,
                received_ack_failed_events: 0,
            });
        }
        let message_id = reaction_message_id(&line.payload)?;
        let request_id = reaction_request_id(&line.payload, &message_id);
        let chat_id = reaction_optional_string(&line.payload, "chat_id");
        let sender_open_id = reaction_sender_open_id(&line.payload);
        let payload_json = serde_json::to_string(&line.payload)
            .map_err(|_| AxonError::runtime_failure("Feishu reaction payload 序列化失败"))?;
        let _append = RemoteFeishuGatewayEventSourceWriterRuntime::append_raw_reaction(
            RemoteFeishuGatewayRawReactionAppendInput {
                event_source_path: input.event_source_file,
                request_id,
                message_id,
                chat_id,
                sender_open_id,
                payload_json,
                enqueued_at_ms: now_ms(),
            },
        )?;
        return Ok(RealSdkPayloadAppendReport {
            input_payloads: 1,
            mapped_events: 1,
            appended_events: 1,
            received_acked_events: 0,
            received_ack_failed_events: 0,
        });
    }
    let should_ack_callback = matches!(
        line.kind.as_str(),
        SDK_LOOP_KIND_MESSAGE | SDK_LOOP_KIND_CARD_ACTION | SDK_LOOP_KIND_CARD_ACTION_KEBAB
    );
    let events = map_payload_lines(vec![line], input.text_context.as_ref())
        .map_err(|_| AxonError::runtime_failure("Feishu SDK loop payload 映射失败"))?;
    let mapped_events = events.len();
    let ack = callback_ack_report(should_ack_callback, mapped_events);
    let report = RemoteFeishuGatewaySdkLoopAdapterRuntime::run_events_with_interceptor(
        RemoteFeishuGatewaySdkLoopRunnerInput {
            event_source_path: input.event_source_file,
            session_id: SessionId(input.session_id),
            client_id: REMOTE_FEISHU_SDK_LOOP_CLIENT_ID.to_owned(),
            enqueued_at_ms: now_ms(),
            events,
            media_download: None,
        },
        |event| try_steer(event),
    )?;
    Ok(RealSdkPayloadAppendReport {
        input_payloads: report.input_events,
        mapped_events,
        appended_events: report.appended_events,
        received_acked_events: ack.acked,
        received_ack_failed_events: ack.failed,
    })
}

fn reaction_request_id(payload: &Value, message_id: &str) -> String {
    let event = payload.get("event").unwrap_or(payload);
    event
        .get("event_id")
        .or_else(|| payload.get("event_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(message_id)
        .to_owned()
}

fn reaction_optional_string(payload: &Value, key: &str) -> Option<String> {
    let event = payload.get("event").unwrap_or(payload);
    event
        .get(key)
        .or_else(|| payload.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn reaction_sender_open_id(payload: &Value) -> Option<String> {
    let event = payload.get("event").unwrap_or(payload);
    event
        .get("operator")
        .and_then(|value| value.get("open_id"))
        .or_else(|| event.get("user_id").and_then(|value| value.get("open_id")))
        .or_else(|| {
            event
                .get("operator_id")
                .and_then(|value| value.get("open_id"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ReceivedAckReport {
    acked: usize,
    failed: usize,
}

fn callback_ack_report(should_ack_callback: bool, mapped_events: usize) -> ReceivedAckReport {
    if should_ack_callback && mapped_events > 0 {
        return ReceivedAckReport {
            acked: 1,
            failed: 0,
        };
    }
    ReceivedAckReport::default()
}

fn reaction_operator_is_app_or_bot(payload: &Value) -> bool {
    let event = payload.get("event").unwrap_or(payload);
    event
        .get(SDK_LOOP_OPERATOR_TYPE_KEY)
        .or_else(|| payload.get(SDK_LOOP_OPERATOR_TYPE_KEY))
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|operator_type| matches!(operator_type, "app" | "bot"))
}

fn reaction_message_id(payload: &Value) -> AxonResult<String> {
    let event = payload.get("event").unwrap_or(payload);
    event
        .get("message_id")
        .or_else(|| payload.get("message_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(AxonError::runtime_failure(
            "Feishu reaction payload 缺少 message_id",
        ))
}

fn sdk_loop_kind(value: &Value) -> AxonResult<String> {
    let event = value.get("event").unwrap_or(value);
    if event.get("message").is_some() {
        return Ok(SDK_LOOP_KIND_MESSAGE.to_owned());
    }
    if event.get("action").is_some() {
        return Ok(SDK_LOOP_KIND_CARD_ACTION.to_owned());
    }
    if event.get(SDK_LOOP_REACTION_TYPE_KEY).is_some() {
        return Ok(SDK_LOOP_KIND_REACTION.to_owned());
    }
    if ignored_sdk_loop_event(value, event) {
        return Ok(SDK_LOOP_KIND_IGNORED.to_owned());
    }
    Err(AxonError::runtime_failure(
        "Feishu SDK loop payload 类型无法识别",
    ))
}

fn ignored_sdk_loop_event(root: &Value, event: &Value) -> bool {
    let event_type = root
        .get(SDK_LOOP_EVENT_TYPE_KEY)
        .or_else(|| {
            root.get(SDK_LOOP_HEADER_KEY)
                .and_then(|header| header.get(SDK_LOOP_EVENT_TYPE_KEY))
        })
        .or_else(|| event.get(SDK_LOOP_EVENT_TYPE_KEY))
        .and_then(Value::as_str)
        .unwrap_or_default();
    matches!(
        event_type,
        "im.message.message_read_v1"
            | "im.chat.member.bot.added_v1"
            | "im.chat.member.bot.deleted_v1"
            | "im.chat.access_event.bot_p2p_chat_entered_v1"
            | "im.message.recalled_v1"
            | "drive.notice.comment_add_v1"
            | "vc.bot.meeting_invited_v1"
    )
}

pub(super) fn map_payload_lines(
    payloads: Vec<RemoteFeishuSdkLoopPayloadLine>,
    text_context: Option<&RemoteFeishuGatewaySdkPayloadTextContext>,
) -> Result<Vec<RemoteFeishuGatewaySdkLoopEvent>, String> {
    payloads
        .into_iter()
        .map(|line| {
            let raw_payload = line.payload.to_string();
            match line.kind.as_str() {
                SDK_LOOP_KIND_MESSAGE => {
                    RemoteFeishuGatewaySdkPayloadMapperRuntime::map_message_payload_with_text_context(
                        &raw_payload,
                        text_context,
                    )
                }
                SDK_LOOP_KIND_CARD_ACTION | SDK_LOOP_KIND_CARD_ACTION_KEBAB => {
                    RemoteFeishuGatewaySdkPayloadMapperRuntime::map_card_action_payload(
                        &raw_payload,
                    )
                }
                SDK_LOOP_KIND_REACTION => {
                    RemoteFeishuGatewaySdkPayloadMapperRuntime::map_reaction_payload(&raw_payload)
                }
                _ => {
                    return Err(
                        "Feishu SDK payload kind 仅支持 message、cardAction 或 reaction".to_owned(),
                    );
                }
            }
            .map_err(|error| format!("{error:?}"))
        })
        .collect()
}

#[cfg(test)]
#[path = "remote_feishu_sdk_loop_tests.rs"]
mod tests;
