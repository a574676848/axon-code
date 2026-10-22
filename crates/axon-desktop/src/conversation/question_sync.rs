//! 车道待决提问的对账查询。
//!
//! 事实源是网关的 `GET /api/v1/lanes/{laneId}/questions`：库里存的是问答记录的
//! 权威状态，窗口重新聚焦时用它校正本地投影。
//!
//! 网关载荷的字段名与前端消费的形状并不相同（`state` 而不是 `status`、`answer`
//! 而不是 `answers`，也不带 `partId`），因此适配在 Rust 侧一次做完：前端只消费
//! 已经对齐的结构，不必各自解释同一份协议。
//!
//! 本模块**只读**：它不写任何本地状态。请求失败时调用方保留本地事实，界面因此
//! 不会因为一次网络抖动把待决提问抹成空。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::{LANES_PATH, UserAnswerItem, UserQuestionItem};
use crate::gateway;

/// 单次对账拉取的条数上限。
const MAX_QUESTION_LIMIT: u32 = 200;
/// 默认拉取条数；网关侧的清单上限与它同量级。
const DEFAULT_QUESTION_LIMIT: u32 = 50;
/// 派生 partId 的前缀：网关载荷不含该字段，按 callId 派生一个稳定标识。
const DERIVED_PART_PREFIX: &str = "question:";
/// 网关状态到界面状态的映射中，代表「已结算」的那一档。
const STATUS_SETTLED: &str = "settled";
/// 界面状态下需要用户作答的两档。
const STATUS_PENDING: &str = "pending";
const STATUS_SUSPENDED: &str = "suspended";

/// 对账结果里的一条提问，字段形状与前端 `PendingQuestion` 一一对应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionSnapshot {
    /// 稳定 part ID；网关未下发时按 callId 派生。
    pub part_id: String,
    /// 工具调用 ID。
    pub call_id: String,
    /// 界面状态：pending、extended、suspended 或 settled。
    pub status: String,
    /// 提出的问题列表。
    pub questions: Vec<UserQuestionItem>,
    /// 已作答内容；未作答时为空。
    pub answers: Vec<UserAnswerItem>,
    /// 挂起交接文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    /// 截止时间毫秒时间戳。
    pub deadline_at_ms: i64,
}

/// 网关单条问答记录的原始形状。
///
/// 字段名逐字段对齐网关源码：`state` / `answer` / `questionId` 都是服务端口径，
/// 客户端不改名，只在适配层翻译一次。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawQuestion {
    /// 领域问题 ID。
    #[serde(default)]
    question_id: String,
    /// 工具调用 ID。
    #[serde(default)]
    call_id: String,
    /// 网关状态：pending、answered、suspended、cancelled 或 expired。
    #[serde(default)]
    state: String,
    /// 截止时间毫秒时间戳。
    #[serde(default)]
    deadline_at_ms: i64,
    /// 结构化问题 JSON；缺字段时按空列表处理。
    #[serde(default)]
    questions: Vec<UserQuestionItem>,
    /// 挂起交接文本。
    #[serde(default)]
    handoff: Option<String>,
    /// 作答内容；未作答时为 null。
    #[serde(default)]
    answer: Option<serde_json::Value>,
    /// 稳定 part ID；网关当前不下发，保留给后续协议补齐。
    #[serde(default)]
    part_id: Option<String>,
}

/// 网关车道问答载荷。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaneQuestionsPayload {
    /// 该车道下的问答记录。
    #[serde(default)]
    questions: Vec<RawQuestion>,
}

/// 网关状态翻译成界面状态。
///
/// 界面只区分「还在等回答」与「已经不需要回答」，因此三种终态（answered、
/// cancelled、expired）统一收敛成 `settled`：它们都代表这一条不再占用待答位。
fn map_state(state: &str) -> &'static str {
    match state {
        "pending" => STATUS_PENDING,
        "suspended" => STATUS_SUSPENDED,
        // 未知状态一律按终态处理：把状态不明的东西留在待答位上，用户会看到一个
        // 永远点不完的提示，比少提示一次更糟。
        _ => STATUS_SETTLED,
    }
}

/// 解析结构化作答内容。
///
/// 网关把库里的 `answer_json` 先解析成 JSON 再下发，因此这里拿到的是数组；
/// 形状不符时返回空列表，而不是让整次对账失败。
fn parse_answers(answer: Option<&serde_json::Value>) -> Vec<UserAnswerItem> {
    answer
        .and_then(|value| serde_json::from_value::<Vec<UserAnswerItem>>(value.clone()).ok())
        .unwrap_or_default()
}

/// 把一条网关记录适配成前端消费的形状。
fn adapt(raw: RawQuestion) -> QuestionSnapshot {
    let part_id = raw
        .part_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{DERIVED_PART_PREFIX}{}",
                derive_part_key(&raw.call_id, &raw.question_id)
            )
        });
    let answers = parse_answers(raw.answer.as_ref());
    // 终态记录即使网关未带状态也按已结算处理，避免残留的旧记录继续占待答位。
    let status = if raw.state.trim().is_empty() {
        STATUS_SETTLED
    } else {
        map_state(raw.state.trim())
    };
    QuestionSnapshot {
        part_id,
        call_id: raw.call_id,
        status: status.to_string(),
        questions: raw.questions,
        answers,
        handoff: raw.handoff,
        deadline_at_ms: raw.deadline_at_ms,
    }
}

/// 派生 partId 时用的键：优先工具调用 ID，它才有回答接口的归属；调用 ID 缺失的
/// 畸形记录退到领域问题 ID，最后用占位串，绝不产出空标识。
fn derive_part_key(call_id: &str, question_id: &str) -> String {
    let call_id = call_id.trim();
    if !call_id.is_empty() {
        return call_id.to_string();
    }
    let question_id = question_id.trim();
    if !question_id.is_empty() {
        return question_id.to_string();
    }
    "unknown".to_string()
}

/// 列出某条车道下的问答记录。
///
/// `state` 给定时只取该状态；缺省时不加状态过滤。**默认不加过滤是有意的**：
/// 网关的 `state` 只能取单值，而「待决」与「挂起」是两种状态，只查 pending 会把
/// 挂起项误判成已结算。对账需要看到全部状态才能让本地投影真正收敛。
pub fn lane_questions(
    lane_id: &str,
    state: Option<&str>,
    limit: Option<u32>,
) -> Result<Vec<QuestionSnapshot>, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let limit = limit
        .unwrap_or(DEFAULT_QUESTION_LIMIT)
        .clamp(1, MAX_QUESTION_LIMIT);
    let mut path = format!("{LANES_PATH}/{lane_id}/questions?limit={limit}");
    if let Some(state) = state.map(str::trim).filter(|value| !value.is_empty()) {
        path.push_str(&format!("&state={state}"));
    }
    let payload = gateway::get_json_at(&path)?;
    let parsed: LaneQuestionsPayload =
        serde_json::from_value(payload).map_err(|error| format!("解析车道问答失败：{error}"))?;
    Ok(parsed.questions.into_iter().map(adapt).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_lane_id_is_rejected_before_any_request() {
        assert!(super::lane_questions("  ", None, None).is_err());
    }

    #[test]
    fn gateway_states_collapse_into_the_two_interface_buckets() {
        assert_eq!(map_state("pending"), "pending");
        assert_eq!(map_state("suspended"), "suspended");
        // 三种终态都不再占用待答位。
        assert_eq!(map_state("answered"), "settled");
        assert_eq!(map_state("cancelled"), "settled");
        assert_eq!(map_state("expired"), "settled");
        assert_eq!(map_state("未来新增的状态"), "settled");
    }

    #[test]
    fn gateway_fields_are_adapted_without_dropping_local_shape() {
        let raw: RawQuestion = serde_json::from_value(serde_json::json!({
            "questionId": "question-1",
            "callId": "call-9",
            "state": "answered",
            "deadlineAtMs": 1_700_000_000_000_i64,
            "questions": [{ "id": "q1", "question": "是否部署？" }],
            "handoff": "我先停在这里",
            "answer": [{ "id": "q1", "selected": ["同意"], "custom": null }],
        }))
        .expect("网关载荷必须按真实字段名解析");

        let snapshot = adapt(raw);
        assert_eq!(snapshot.call_id, "call-9");
        assert_eq!(snapshot.status, "settled");
        assert_eq!(snapshot.part_id, "question:call-9");
        assert_eq!(snapshot.questions.len(), 1);
        assert_eq!(snapshot.answers.len(), 1);
        assert_eq!(snapshot.answers[0].selected, vec!["同意".to_string()]);
        assert_eq!(snapshot.handoff.as_deref(), Some("我先停在这里"));
        assert_eq!(snapshot.deadline_at_ms, 1_700_000_000_000);
    }

    #[test]
    fn missing_or_malformed_answer_fields_degrade_instead_of_failing() {
        let raw: RawQuestion = serde_json::from_value(serde_json::json!({
            "callId": "call-10",
            "state": "pending",
        }))
        .expect("缺字段的载荷必须能解析");
        let snapshot = adapt(raw);
        assert_eq!(snapshot.status, "pending");
        assert!(snapshot.answers.is_empty());
        assert!(snapshot.questions.is_empty());
        assert_eq!(snapshot.deadline_at_ms, 0);
        assert!(snapshot.handoff.is_none());

        // 作答内容形状不符（对象而不是数组）时只丢这一项，不让整次对账失败。
        let malformed: RawQuestion = serde_json::from_value(serde_json::json!({
            "callId": "call-11",
            "state": "answered",
            "answer": { "id": "q1" },
        }))
        .expect("异常作答形状必须能解析");
        assert!(adapt(malformed).answers.is_empty());
    }

    #[test]
    fn gateway_supplied_part_id_wins_over_the_derived_one() {
        let raw: RawQuestion = serde_json::from_value(serde_json::json!({
            "callId": "call-12",
            "state": "pending",
            "partId": "part:call-12",
        }))
        .expect("载荷必须能解析");
        assert_eq!(adapt(raw).part_id, "part:call-12");
    }

    #[test]
    fn derived_part_key_never_ends_up_empty() {
        assert_eq!(derive_part_key("call-13", "question-13"), "call-13");
        assert_eq!(derive_part_key("  ", "question-14"), "question-14");
        assert_eq!(derive_part_key("", ""), "unknown");
    }
}
