//! 通知资格与文案：一轮结束要不要提醒、提醒时写什么。
//!
//! 这里只做纯判定，窗口状态与系统能力留在 `desktop_shell`：
//! 「要不要弹」与「弹什么」都能被单测逐条钉住，不必真开一个窗口。
//!
//! 分流口径（与产品对齐）：
//! - **前台**（窗口可见且聚焦）只推界面标识，不发系统通知；
//! - **后台**发系统通知：标题取会话标题，正文取 AI 回复正文的摘要；
//! - 回复为空且本轮成功时不弹，只有标题的通知等于骗人点开；
//!   失败与中断改为写明状态，必须被看见。

use std::collections::{HashMap, HashSet};

use super::StreamEvent;

/// 收口事件类型，与后端一致。
pub(super) const EVENT_TURN_COMPLETED: &str = "turn_completed";
/// 运行过程事件类型：其中承载轮次生命周期。
pub(super) const EVENT_WIRE: &str = "wire_event";
/// 提问类提示的状态前缀。
const QUESTION_PREFIX: &str = "question_";
/// 通知标题的最大字符数：系统通知只露一行标题，超出部分会被系统自己切掉。
const TITLE_LIMIT: usize = 48;
/// 通知正文的最大字符数：它不是阅读区，120 字已经能说完一条结论。
const BODY_LIMIT: usize = 120;
/// 截断时补的省略号：单个字符，不写成三个点。
const ELLIPSIS: char = '…';

/// 一次轮到用户知道的提示。
#[derive(Debug)]
pub(super) struct TurnNotice {
    /// 提示标识：轮次标识或提问标识，用于去重。
    pub(super) id: String,
    /// 终态：completed / failed / interrupted，以及 question_pending 等。
    pub(super) status: String,
    /// 状态标签：本轮已完成 / 本轮执行失败 / 模型需要您的确认。
    pub(super) title: String,
    /// 可读正文：AI 回复正文的压平摘要，或提问正文；没有可读内容时为空串。
    pub(super) body: String,
}

impl TurnNotice {
    /// 是否是「有事等你做」的提问提示。
    fn is_question(&self) -> bool {
        self.status.starts_with(QUESTION_PREFIX)
    }
}

/// 一条提示的标题与正文；`None` 表示这条不该弹。
///
/// 标题只写会话标题：状态已经在正文里说清，一条通知里两个主体只会让人读两遍。
pub(super) fn notification_copy(
    notice: &TurnNotice,
    session_title: Option<&str>,
) -> Option<(String, String)> {
    let session = session_title
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if notice.is_question() {
        // 提问要的是用户的一个动作，标题必须说清是哪种动作，因此带状态标签。
        let title = compose_title(session, &notice.title);
        let body = if notice.body.is_empty() {
            "请回到 Axon 作答".to_owned()
        } else {
            notice.body.clone()
        };
        return Some((clip(&title, TITLE_LIMIT), clip(&body, BODY_LIMIT)));
    }
    let title = session.unwrap_or(&notice.title);
    let body = match (notice.body.as_str(), notice.status.as_str()) {
        // 成功：正文就是回复摘要。
        ("", "completed") => return None,
        ("", _) => format!("{}，可在 Axon 中查看详情", notice.title),
        // 失败与中断：状态说明贴在摘要之前，摘要再长也不能把它挤掉。
        (body, "completed") => body.to_owned(),
        (body, _) => format!("{}：{body}", notice.title),
    };
    Some((clip(title, TITLE_LIMIT), clip(&body, BODY_LIMIT)))
}

/// 会话名在前的标题：只写状态标签时，用户得点进去才知道是哪个会话。
fn compose_title(session: Option<&str>, base: &str) -> String {
    match session {
        Some(session) => format!("{session} · {base}"),
        None => base.to_owned(),
    }
}

/// 压平空白：多行正文直接贴进通知会带出一串空格，读起来是断裂的。
fn flatten(text: &str) -> String {
    let mut flattened = String::with_capacity(text.len());
    let mut in_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            in_space = true;
            continue;
        }
        if in_space && !flattened.is_empty() {
            flattened.push(' ');
        }
        in_space = false;
        flattened.push(character);
    }
    flattened
}

/// 压平并按正文上限收口。
pub(super) fn compact_body(text: &str) -> String {
    clip(&flatten(text), BODY_LIMIT)
}

/// 按字符上限收口，能落在句读上就落在句读上。
///
/// 截断点太靠前（不足半程）时宁可硬切：一个逗号之后的半句话不算摘要。
/// 收口后去掉悬空标点，再补一个省略号；原文截断处本就是省略号时不重复补。
fn clip(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let head: Vec<char> = text.chars().take(limit).collect();
    let floor = (limit / 2).max(1);
    let mut cut = head.len();
    for (index, character) in head.iter().enumerate().rev() {
        if !is_clause_break(*character) {
            continue;
        }
        if index + 1 >= floor {
            cut = index + 1;
        }
        break;
    }
    let mut clipped: String = head[..cut].iter().collect();
    while clipped.ends_with(is_clip_tail) {
        clipped.pop();
    }
    if !clipped.is_empty() && !clipped.ends_with(ELLIPSIS) {
        clipped.push(ELLIPSIS);
    }
    clipped
}

/// 可以作为收口落点的句读。ASCII 句点不算：小数点与缩写都会被它切开。
const fn is_clause_break(character: char) -> bool {
    matches!(
        character,
        '。' | '！' | '？' | '；' | '，' | '、' | '!' | '?' | ';' | ','
    )
}

/// 收口时要去掉的悬空标点与空白。
const fn is_clip_tail(character: char) -> bool {
    character.is_whitespace() || is_clause_break(character) || matches!(character, '：' | ':')
}

/// 从一帧事件里判断是否构成「一轮结束」。
///
/// 收口事件是首选；但订阅端未必收得到终态帧，而轮次生命周期是 durable 的
/// 运行事件、稳定到达，因此两条路都认，避免通知整体失效。
fn turn_notice(event_type: &str, payload_json: &str) -> Option<TurnNotice> {
    if event_type == EVENT_TURN_COMPLETED {
        let payload: serde_json::Value = serde_json::from_str(payload_json).unwrap_or_default();
        let status = payload
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("completed");
        let id = payload
            .get("interactionId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let body = payload
            .get("response")
            .and_then(serde_json::Value::as_str)
            .map(compact_body)
            .unwrap_or_default();
        return Some(TurnNotice {
            id,
            status: status.to_owned(),
            title: status_title(status).to_owned(),
            body,
        });
    }
    if event_type != EVENT_WIRE {
        return None;
    }
    let envelope: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    let kind = envelope.get("kind")?;
    if kind.get("type").and_then(serde_json::Value::as_str) != Some("turnLifecycle") {
        return None;
    }
    let payload = kind.get("payload")?;
    if payload.get("phase").and_then(serde_json::Value::as_str) != Some("regularTurn") {
        return None;
    }
    let status = payload
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if status != "completed" && status != "failed" {
        return None;
    }
    // 生命周期事件不带正文：正文由订阅流上的回复帧补，缺了就走空正文口径。
    Some(TurnNotice {
        id: envelope
            .get("interactionId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{}:{status}",
                    payload
                        .get("turnId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                )
            }),
        status: status.to_owned(),
        title: status_title(status).to_owned(),
        body: String::new(),
    })
}

/// 终态到提示标题。
const fn status_title(status: &str) -> &'static str {
    match status.as_bytes() {
        b"failed" => "本轮执行失败",
        b"interrupted" => "本轮已中断",
        _ => "本轮已完成",
    }
}

/// 问答提问的通知标题。
fn question_title(status: &str) -> &'static str {
    if status == "suspended" {
        "任务已挂起，等待回复"
    } else {
        "模型需要您的确认"
    }
}

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
            StreamEvent::Subscribed { subscribe_id, .. }
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
        // 终态按 Interaction 去重；订阅标识只有 `interaction:` 与 `session-turns:` 两种。
        let key = notice.id.clone();
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
    let title = question_title(status).to_owned();
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
        .unwrap_or_default();

    Some(TurnNotice {
        id: call_id.to_owned(),
        status: format!("{QUESTION_PREFIX}{status}"),
        title,
        body: compact_body(body),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn notice(status: &str, title: &str, body: &str) -> TurnNotice {
        TurnNotice {
            id: "i-1".to_owned(),
            status: status.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
        }
    }

    #[test]
    fn notification_body_is_flattened_and_bounded() {
        assert_eq!(
            compact_body("第一行\n\n第二行\t 第三行"),
            "第一行 第二行 第三行"
        );
        assert_eq!(compact_body("   "), "");
        let long = compact_body(&"长".repeat(BODY_LIMIT + 10));
        assert_eq!(long.chars().count(), BODY_LIMIT + 1);
        assert!(long.ends_with(ELLIPSIS));
    }

    #[test]
    fn clip_prefers_clause_breaks_and_never_doubles_the_ellipsis() {
        assert_eq!(clip("简短结论", BODY_LIMIT), "简短结论");

        // 超长时在句读处收口，截断点之后的句子不进入摘要。
        let sentence = "第一句话说得足够长以便越过句读下限。第二句话不应该出现在摘要里。";
        let clipped = clip(sentence, 20);
        assert!(clipped.starts_with("第一句话"), "应在句读处收口：{clipped}");
        assert!(
            !clipped.contains("第二句"),
            "截断点之后的内容不得混入：{clipped}"
        );
        assert!(clipped.ends_with(ELLIPSIS), "截断必须补省略号：{clipped}");

        // 没有句读可依时硬切，并收掉悬空标点。
        assert_eq!(
            clip(&"无".repeat(BODY_LIMIT + 10), BODY_LIMIT)
                .chars()
                .count(),
            BODY_LIMIT + 1
        );
        assert_eq!(clip("一，二，三", 2), "一…");

        // 原文截断处已经是省略号：不叠加第二个。
        assert_eq!(clip("结论……", 3), "结论…");
    }

    #[test]
    fn completed_notification_carries_session_title_and_reply_summary() {
        let completed = notice("completed", "本轮已完成", "三条门禁都跑完了");
        let (title, body) = notification_copy(&completed, Some("梳理设计规范")).expect("应发通知");
        assert_eq!(title, "梳理设计规范");
        assert_eq!(body, "三条门禁都跑完了");

        // 没有会话名时标题回落到状态标签，正文不变。
        let (title, _) = notification_copy(&completed, None).expect("应发通知");
        assert_eq!(title, "本轮已完成");
    }

    #[test]
    fn completed_turn_without_reply_stays_silent_but_failures_speak() {
        let silent = notice("completed", "本轮已完成", "");
        assert!(
            notification_copy(&silent, Some("会话甲")).is_none(),
            "没有回复正文的成功轮次不得弹空通知"
        );

        let failed = notice("failed", "本轮执行失败", "");
        let (_, body) = notification_copy(&failed, Some("会话甲")).expect("失败必须通知");
        assert_eq!(body, "本轮执行失败，可在 Axon 中查看详情");

        let interrupted = notice("interrupted", "本轮已中断", "");
        let (_, body) = notification_copy(&interrupted, Some("会话甲")).expect("中断必须通知");
        assert_eq!(body, "本轮已中断，可在 Axon 中查看详情");
    }

    #[test]
    fn failure_prefix_survives_truncation() {
        let failed = notice("failed", "本轮执行失败", &"错".repeat(BODY_LIMIT + 20));
        let (_, body) = notification_copy(&failed, Some("会话甲")).expect("失败必须通知");
        assert!(
            body.starts_with("本轮执行失败："),
            "状态说明先于摘要：{body}"
        );
        assert_eq!(body.chars().count(), BODY_LIMIT + 1);
        assert!(body.ends_with(ELLIPSIS));
    }

    #[test]
    fn question_notice_keeps_the_session_prefix_and_never_goes_empty() {
        let question = notice("question_pending", "模型需要您的确认", "");
        let (title, body) = notification_copy(&question, Some("会话甲")).expect("提问必须通知");
        assert_eq!(title, "会话甲 · 模型需要您的确认");
        assert_eq!(body, "请回到 Axon 作答");
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
    fn turn_notice_recognises_both_terminal_facts() {
        let completed = turn_notice(
            EVENT_TURN_COMPLETED,
            r#"{"interactionId":"i-1","status":"failed","response":"出错了"}"#,
        )
        .expect("收口事件应构成提示");
        assert_eq!(completed.id, "i-1");
        assert_eq!(completed.title, "本轮执行失败");
        assert_eq!(completed.body, "出错了");

        // 生命周期事件本身不带正文：正文由订阅流上的回复帧补。
        let lifecycle = turn_notice(
            EVENT_WIRE,
            r#"{"kind":{"type":"turnLifecycle","payload":{"phase":"regularTurn","status":"completed","turnId":"t-9","title":"本轮完成"}}}"#,
        )
        .expect("生命周期终态应构成提示");
        assert_eq!(lifecycle.id, "t-9:completed");
        assert_eq!(lifecycle.title, "本轮已完成");
        assert_eq!(lifecycle.body, "");

        assert!(turn_notice(EVENT_WIRE, r#"{"kind":{"type":"turnLifecycle","payload":{"phase":"regularTurn","status":"running"}}}"#).is_none());
        assert!(turn_notice("runtime_observation", "{}").is_none());
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
