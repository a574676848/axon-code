//! 用户问答答案路由的统一出口（UQ-005）。
//!
//! HTTP 路由与 Daemon IPC 命令必须给出同一份状态码与错误码：两者都经过这里，
//! 因此不存在「HTTP 拒绝、IPC 放行」的分叉。答案的校验、CAS 结算、二选一投递、
//! 事件发射与对账全部收敛在 [`UserQuestionAnswerService`] 内。

use axon_domain::UserAnswerItem;
use serde_json::Value;

use super::answer_service::{AnswerError, UserQuestionAnswerService};

/// HTTP 请求体缺省渠道。
const DEFAULT_CHANNEL: &str = "desktop";

/// 一次答案路由的结果：状态码、错误码（成功时为空）与响应载荷。
#[derive(Debug, Clone)]
pub struct AnswerRouteOutcome {
    /// HTTP 语义状态码；IPC 侧据此给出等价的成功/失败判定。
    pub status_code: u16,
    /// 稳定错误码；成功时为 `None`。
    pub code: Option<&'static str>,
    /// 响应载荷。
    pub payload: Value,
}

impl AnswerRouteOutcome {
    /// 本次路由是否成功结算。
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.code.is_none()
    }

    fn failure(status_code: u16, code: &'static str, message: String) -> Self {
        Self {
            status_code,
            code: Some(code),
            payload: serde_json::json!({ "error": message, "code": code }),
        }
    }
}

/// 结算一次作答。
pub fn route_answer(
    service: &UserQuestionAnswerService,
    lane_id: &str,
    call_id: &str,
    answers: Vec<UserAnswerItem>,
    channel: &str,
) -> AnswerRouteOutcome {
    match service.submit_answer(lane_id, call_id, answers, channel) {
        Ok(record) => AnswerRouteOutcome {
            status_code: 200,
            code: None,
            payload: serde_json::json!({
                "laneId": record.lane_id,
                "callId": record.tool_call_id,
                "questionId": record.question_id,
                "state": record.state.as_str(),
                "delivered": record.delivered,
                "deliveryMode": record.delivery_mode,
            }),
        },
        Err(AnswerError::BadAnswer(message)) => {
            AnswerRouteOutcome::failure(400, "BAD_ANSWER", message)
        }
        Err(AnswerError::UnknownCall(message)) => {
            AnswerRouteOutcome::failure(404, "UNKNOWN_CALL", message)
        }
        Err(AnswerError::AlreadySettled(message)) => {
            AnswerRouteOutcome::failure(409, "ALREADY_SETTLED", message)
        }
        Err(AnswerError::LaneMismatch(message)) => {
            AnswerRouteOutcome::failure(400, "LANE_MISMATCH", message)
        }
        Err(AnswerError::Internal(message)) => {
            AnswerRouteOutcome::failure(500, "INTERNAL_ERROR", message)
        }
    }
}

/// 解析 HTTP 答案请求体；请求体不可解析时按 `BAD_ANSWER` 拒绝。
pub fn parse_answer_body(body: &[u8]) -> Result<(Vec<UserAnswerItem>, String), AnswerRouteOutcome> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AnswerRequestBody {
        answers: Vec<UserAnswerItem>,
        #[serde(default = "default_channel")]
        channel: String,
    }

    fn default_channel() -> String {
        DEFAULT_CHANNEL.to_string()
    }

    serde_json::from_slice::<AnswerRequestBody>(body)
        .map(|request| (request.answers, request.channel))
        .map_err(|error| {
            AnswerRouteOutcome::failure(400, "BAD_ANSWER", format!("解析答案请求失败: {error}"))
        })
}
