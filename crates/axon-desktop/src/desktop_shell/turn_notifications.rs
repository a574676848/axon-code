//! 通知资格与正文归属：历史回放不产生新结果提示。

use std::collections::{HashMap, HashSet};

use super::{EVENT_WIRE, StreamEvent, TurnNotice, compact_body, turn_notice};

#[derive(Default)]
pub(super) struct NotificationState {
    replies: HashMap<String, String>,
    finished: HashSet<String>,
}

impl NotificationState {
    pub(super) fn observe(&mut self, event: &StreamEvent) -> Option<TurnNotice> {
        let (subscribe_id, event_type, payload_json) = match event {
            StreamEvent::Frame {
                replay: false,
                subscribe_id,
                event_type,
                payload_json,
                ..
            } => (subscribe_id, event_type, payload_json),
            StreamEvent::Subscribed { subscribe_id }
            | StreamEvent::Failure { subscribe_id, .. } => {
                self.replies.remove(subscribe_id);
                return None;
            }
            StreamEvent::Frame { replay: true, .. } => return None,
        };
        if let Some(reply) = assistant_reply(event_type, payload_json) {
            self.replies.insert(subscribe_id.clone(), reply);
        }
        if let Some(notice) = question_notice(event_type, payload_json) {
            let key = format!("question:{}", notice.id);
            if !self.finished.insert(key) {
                return None;
            }
            return Some(notice);
        }
        let mut notice = turn_notice(event_type, payload_json)?;
        let reply = self.replies.remove(subscribe_id);
        // 主会话的两种终态共用 Interaction ID；子代理的轮次标识按会话隔离。
        let key = if subscribe_id.starts_with("session:") {
            format!("{subscribe_id}:{}", notice.id)
        } else {
            notice.id.clone()
        };
        if notice.id.is_empty() || !self.finished.insert(key) {
            return None;
        }
        if let Some(reply) = reply {
            notice.body = reply;
        }
        Some(notice)
    }
}

fn assistant_reply(event_type: &str, payload_json: &str) -> Option<String> {
    if event_type != EVENT_WIRE {
        return None;
    }
    let envelope: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    let kind = envelope.get("kind")?;
    if kind.get("type")?.as_str()? != "assistantMessage" {
        return None;
    }
    let content = kind.get("payload")?.get("content")?.as_str()?;
    (!content.trim().is_empty()).then(|| compact_body(content))
}

fn question_notice(event_type: &str, payload_json: &str) -> Option<TurnNotice> {
    if event_type != EVENT_WIRE {
        return None;
    }
    let envelope: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    let kind = envelope.get("kind")?;
    if kind.get("type")?.as_str()? != "userQuestion" {
        return None;
    }
    let payload = kind.get("payload")?;
    let status = payload.get("status")?.as_str()?;
    if status != "pending" && status != "suspended" {
        return None;
    }
    let call_id = payload.get("callId")?.as_str()?;
    let title = super::question_title(status).to_string();
    let body = payload
        .get("handoff")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            payload
                .get("questions")
                .and_then(serde_json::Value::as_array)?
                .first()?
                .get("question")?
                .as_str()
        })
        .unwrap_or("请点击查看并作答");

    Some(TurnNotice {
        id: call_id.to_string(),
        status: format!("question_{status}"),
        title,
        body: compact_body(body),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_shell::EVENT_TURN_COMPLETED;

    fn frame(
        subscribe_id: &str,
        event_type: &str,
        payload: serde_json::Value,
        replay: bool,
    ) -> StreamEvent {
        StreamEvent::Frame {
            subscribe_id: subscribe_id.to_owned(),
            event_type: event_type.to_owned(),
            payload_json: payload.to_string(),
            cursor: None,
            replay,
        }
    }

    fn terminal(id: &str, replay: bool) -> StreamEvent {
        frame(
            &format!("interaction:{id}"),
            EVENT_TURN_COMPLETED,
            serde_json::json!({"interactionId": id, "status": "completed", "response": "本轮结果"}),
            replay,
        )
    }

    fn reply(id: &str, content: &str, replay: bool) -> StreamEvent {
        frame(
            &format!("interaction:{id}"),
            EVENT_WIRE,
            serde_json::json!({"kind": {"type": "assistantMessage", "payload": {"content": content}}}),
            replay,
        )
    }

    #[test]
    fn repeated_history_never_notifies_or_overwrites_live_reply() {
        let mut state = NotificationState::default();
        state.observe(&reply("live", "实时结果", false));
        for _ in 0..3 {
            for id in ["old-1", "old-2"] {
                assert!(state.observe(&reply(id, "历史结果", true)).is_none());
                assert!(state.observe(&terminal(id, true)).is_none());
            }
        }
        assert_eq!(
            state
                .observe(&terminal("live", false))
                .expect("实时完成应通知")
                .body,
            "实时结果"
        );
    }

    #[test]
    fn replaying_a_running_turn_does_not_suppress_its_live_completion() {
        let mut state = NotificationState::default();
        state.observe(&reply("running", "旧片段", true));
        assert!(state.observe(&terminal("running", false)).is_some());
    }

    #[test]
    fn interleaved_subscriptions_keep_their_own_reply() {
        let mut state = NotificationState::default();
        state.observe(&reply("first", "第一条回复", false));
        state.observe(&reply("second", "第二条回复", false));
        assert_eq!(
            state
                .observe(&terminal("first", false))
                .expect("第一条通知")
                .body,
            "第一条回复"
        );
        assert_eq!(
            state
                .observe(&terminal("second", false))
                .expect("第二条通知")
                .body,
            "第二条回复"
        );
    }

    #[test]
    fn lifecycle_and_completion_are_deduplicated_even_when_interleaved() {
        let mut state = NotificationState::default();
        let lifecycle = frame(
            "interaction:first",
            EVENT_WIRE,
            serde_json::json!({"interactionId": "first", "kind": {"type": "turnLifecycle", "payload": {
                "phase": "regularTurn", "status": "completed", "turnId": "turn-1"
            }}}),
            false,
        );
        assert!(state.observe(&lifecycle).is_some());
        assert!(state.observe(&terminal("second", false)).is_some());
        assert!(state.observe(&terminal("first", false)).is_none());
        assert!(state.observe(&lifecycle).is_none());
    }

    #[test]
    fn question_event_notifies_once_and_skips_replay() {
        let mut state = NotificationState::default();
        let q_frame = |status: &str, replay: bool| {
            frame(
                "interaction:q1",
                EVENT_WIRE,
                serde_json::json!({
                    "kind": {
                        "type": "userQuestion",
                        "payload": {
                            "partId": "part:q1",
                            "callId": "call-1",
                            "status": status,
                            "questions": [{ "id": "1", "question": "是否部署？" }],
                            "deadlineAtMs": 10000
                        }
                    }
                }),
                replay,
            )
        };

        // 历史回放帧绝不通知
        assert!(state.observe(&q_frame("pending", true)).is_none());

        // 实时帧通知恰好一次
        let notice = state.observe(&q_frame("pending", false)).expect("应通知");
        assert_eq!(notice.title, "模型需要您的确认");
        assert_eq!(notice.body, "是否部署？");

        // 重复帧（例如窗口扩展或重复事件）不再重复通知
        assert!(state.observe(&q_frame("pending", false)).is_none());
        assert!(state.observe(&q_frame("suspended", false)).is_none());
    }
}
