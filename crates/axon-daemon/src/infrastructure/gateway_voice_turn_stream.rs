//! 助手事件流的进程内订阅：把本轮增量与终态投递给语音会话。
//!
//! 语音不新开 WS、不回环 HTTP：这里直接用 `GatewayContext::subscribe` 订阅既有
//! 事件桥，按本次提交的 Interaction 过滤，只把本轮的助手文本与终态取出来。

use super::super::gateway_kernel::{GatewayContext, GatewayEvent, GatewaySubscriptionRequest};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;

/// 订阅线程轮询步长：足够小以让停播及时，又不产生忙等。
const SUBSCRIPTION_POLL: Duration = Duration::from_millis(20);

/// 订阅线程投递给会话的信号；`epoch` 用于丢弃旧轮次的迟到信号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TurnSignal {
    /// 助手文本增量。
    Delta {
        /// 轮次。
        epoch: u32,
        /// 增量文本。
        text: String,
    },
    /// 本轮终态。
    Completed {
        /// 轮次。
        epoch: u32,
        /// 收口回复正文；没有时为空。
        response: Option<String>,
    },
    /// 订阅已断开。
    Closed {
        /// 轮次。
        epoch: u32,
    },
}

/// 解析后、尚未标注轮次的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedEvent {
    Delta(String),
    Completed(Option<String>),
}

/// 拉起本轮的事件订阅线程；`stop` 置位后线程在一个轮询步长内退出。
pub(super) fn spawn_turn_subscription(
    context: GatewayContext,
    workspace_id: String,
    interaction_id: String,
    epoch: u32,
    sink: tokio_mpsc::UnboundedSender<TurnSignal>,
    stop: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let request = GatewaySubscriptionRequest {
            workspace_id,
            interaction_id,
            session_id: None,
            resume_cursor: None,
        };
        let mut stream = match context.subscribe(request) {
            Ok(stream) => stream,
            Err(_) => {
                let _ = sink.send(TurnSignal::Closed { epoch });
                return;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            match stream.recv_timeout(SUBSCRIPTION_POLL) {
                Ok(Some(event)) => {
                    let Some(parsed) = parse_turn_event(&event) else {
                        continue;
                    };
                    let signal = match parsed {
                        ParsedEvent::Delta(text) => TurnSignal::Delta { epoch, text },
                        ParsedEvent::Completed(response) => {
                            TurnSignal::Completed { epoch, response }
                        }
                    };
                    if sink.send(signal).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    let _ = sink.send(TurnSignal::Closed { epoch });
                    return;
                }
            }
        }
    });
}

/// 从网关事件里取出本轮可消费的内容。
fn parse_turn_event(event: &GatewayEvent) -> Option<ParsedEvent> {
    match event.event_type.as_str() {
        "wire_event" => assistant_delta(&event.payload_json).map(ParsedEvent::Delta),
        "turn_completed" => Some(ParsedEvent::Completed(response_of(&event.payload_json))),
        "interaction.status" => {
            let status: String = serde_json::from_str(&event.payload_json).ok()?;
            matches!(status.as_str(), "understood" | "rejected" | "interrupted")
                .then_some(ParsedEvent::Completed(None))
        }
        _ => None,
    }
}

/// 只有增量助手消息才是可朗读文本；非增量消息由终态兜底，不重复。
fn assistant_delta(payload_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    let kind = value.get("kind")?;
    if kind.get("type")?.as_str()? != "assistantMessage" {
        return None;
    }
    let message = kind.get("payload")?;
    if message.get("delta").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    let content = message.get("content")?.as_str()?;
    (!content.is_empty()).then(|| content.to_owned())
}

/// 终态事件里的收口回复；没有正文时为空。
fn response_of(payload_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    value
        .get("response")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{ParsedEvent, parse_turn_event};
    use crate::infrastructure::gateway_kernel::GatewayEvent;

    fn event(event_type: &str, payload_json: &str) -> GatewayEvent {
        GatewayEvent {
            event_type: event_type.to_owned(),
            payload_json: payload_json.to_owned(),
            cursor: None,
            replay: false,
        }
    }

    #[test]
    fn assistant_delta_is_extracted() {
        let payload = r#"{"interactionId":"interaction-1","kind":{"type":"assistantMessage","payload":{"partId":"p1","messageId":"m1","content":"第一句。","delta":true}}}"#;
        assert_eq!(
            parse_turn_event(&event("wire_event", payload)),
            Some(ParsedEvent::Delta("第一句。".to_owned()))
        );
    }

    #[test]
    fn non_delta_and_other_kinds_are_ignored() {
        let complete = r#"{"kind":{"type":"assistantMessage","payload":{"content":"最终消息","delta":false}}}"#;
        assert_eq!(parse_turn_event(&event("wire_event", complete)), None);
        let tool = r#"{"kind":{"type":"toolCall","payload":{"name":"shell"}}}"#;
        assert_eq!(parse_turn_event(&event("wire_event", tool)), None);
    }

    #[test]
    fn terminal_events_carry_the_closing_response() {
        let completed = r#"{"interactionId":"interaction-1","response":"结果已经写进报告。"}"#;
        assert_eq!(
            parse_turn_event(&event("turn_completed", completed)),
            Some(ParsedEvent::Completed(Some(
                "结果已经写进报告。".to_owned()
            )))
        );
        assert_eq!(
            parse_turn_event(&event("interaction.status", r#""understood""#)),
            Some(ParsedEvent::Completed(None))
        );
        assert_eq!(
            parse_turn_event(&event("interaction.status", r#""processing""#)),
            None,
            "非终态不得触发收口"
        );
    }
}
