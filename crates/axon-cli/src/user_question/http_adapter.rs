//! 用户问答 HTTP 路由端点处理器（UQ-005, UQ-107）。

use axon_daemon::GatewayHttpResponse;
use axon_domain::UserQuestionState;
use axon_session::UserQuestionStore;

use super::answer_route::{parse_answer_body, route_answer};
use super::answer_service::UserQuestionAnswerService;

/// 处理 `POST /api/v1/lanes/{laneId}/questions/{callId}/answer` 请求。
///
/// 状态码与错误码由 [`route_answer`] 统一给出，与 Daemon IPC 命令完全一致。
pub fn handle_submit_answer(
    service: &UserQuestionAnswerService,
    lane_id: &str,
    call_id: &str,
    body: &[u8],
) -> GatewayHttpResponse {
    let (answers, channel) = match parse_answer_body(body) {
        Ok(parsed) => parsed,
        Err(outcome) => return GatewayHttpResponse::json(outcome.status_code, &outcome.payload),
    };
    let outcome = route_answer(service, lane_id, call_id, answers, &channel);
    GatewayHttpResponse::json(outcome.status_code, &outcome.payload)
}

/// 处理 `GET /api/v1/lanes/{laneId}/questions` 对账查询请求。
pub fn handle_list_questions(
    store: &dyn UserQuestionStore,
    lane_id: &str,
    query: Option<&str>,
) -> GatewayHttpResponse {
    let state_filter = query.and_then(|q| {
        q.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == "state").then(|| UserQuestionState::parse_state(v))?
        })
    });

    let records = match store.list_questions_by_lane(lane_id, state_filter, 50) {
        Ok(r) => r,
        Err(e) => {
            return GatewayHttpResponse::json(
                500,
                &serde_json::json!({ "error": format!("查询车道问答失败: {e}") }),
            );
        }
    };

    let questions_view: Vec<serde_json::Value> = records
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "questionId": r.question_id,
                "callId": r.tool_call_id,
                "laneId": r.lane_id,
                "state": r.state.as_str(),
                "deadlineAtMs": r.deadline_at_ms,
                "questions": serde_json::from_str::<serde_json::Value>(&r.questions_json).unwrap_or(serde_json::json!([])),
                "handoff": r.handoff_text,
                "answer": r.answer_json.and_then(|a| serde_json::from_str::<serde_json::Value>(&a).ok()),
                "answeredChannel": r.answered_channel,
                "delivered": r.delivered,
                "deliveryMode": r.delivery_mode,
                "createdAtMs": r.created_at_ms,
            })
        })
        .collect();

    GatewayHttpResponse::json(
        200,
        &serde_json::json!({
            "laneId": lane_id,
            "questions": questions_view,
        }),
    )
}
