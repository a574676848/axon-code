//! 提交待决问答的答案。

use crate::conversation::wire::{LANES_PATH, UserAnswerItem};
use serde::{Deserialize, Serialize};

use crate::gateway;

/// 问答结算回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerQuestionReceipt {
    /// 目标车道。
    pub lane_id: String,
    /// 工具调用 ID。
    pub call_id: String,
    /// 领域问题 ID。
    pub question_id: String,
    /// 状态（answered）。
    pub state: String,
    /// 是否已完成投递。
    pub delivered: bool,
    /// 投递方式。
    #[serde(default)]
    pub delivery_mode: Option<String>,
}

/// 提交指定问题的答案。
pub fn answer_question(
    lane_id: &str,
    call_id: &str,
    answers: &[UserAnswerItem],
) -> Result<AnswerQuestionReceipt, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let call_id = call_id.trim();
    if call_id.is_empty() {
        return Err("问题调用标识不能为空".to_string());
    }
    let path = format!("{LANES_PATH}/{lane_id}/questions/{call_id}/answer");
    let payload = gateway::post_json_at(
        &path,
        &serde_json::json!({
            "answers": answers,
            "channel": "desktop"
        }),
    )?;
    serde_json::from_value::<AnswerQuestionReceipt>(payload)
        .map_err(|error| format!("解析问答回执失败：{error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_parameters_are_rejected_before_any_request() {
        assert!(super::answer_question("  ", "call-1", &[]).is_err());
        assert!(super::answer_question("lane-1", "  ", &[]).is_err());
    }
}
