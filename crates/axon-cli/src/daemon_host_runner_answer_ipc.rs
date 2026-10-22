//! 守护进程 IPC 侧的统一答案入口装配与回执构造。
//!
//! 作答只允许有一条入口：校验、CAS 结算、二选一投递与落库后的唤醒全部收敛在
//! `UserQuestionAnswerService` 内，IPC 侧只负责把命令搬进去、把结果按同一份错误码
//! 口径搬回来。因此 HTTP 路由与 Daemon IPC 之间不存在「HTTP 拒绝、IPC 放行」的分叉，
//! 也不存在「只结算、不投递」的第二条路径。

use std::sync::Arc;

use super::DaemonMaintenanceSignal;

/// 守护进程 IPC 侧的统一答案入口装配。
///
/// 答案服务与主 turn 唤醒信号必须同源：作答经这里进入时，校验、CAS 结算、二选一
/// 投递与落库后的唤醒全部走与 HTTP 路由同一份实现。
pub(super) struct DaemonIpcAnswerEntry {
    /// 答案服务单例；与工作台 HTTP 作答入口共用同一个实例。
    pub(super) service: crate::user_question::UserQuestionAnswerService,
    /// 主 turn 唤醒信号：答案落成 durable Interaction 后触发一次领取。
    pub(super) primary_turn_signal: Arc<DaemonMaintenanceSignal>,
}

/// 统一答案入口的 IPC 应答。
///
/// 载荷与错误码整份沿用 HTTP 路由的同一份结果：`code` 与 `error` 同源，不另造口径。
pub(super) fn answer_question_ipc_response(
    service: &crate::user_question::UserQuestionAnswerService,
    request_id: &str,
    lane_id: &str,
    call_id: &str,
    answers: Vec<axon_domain::UserAnswerItem>,
    channel: &str,
) -> axon_protocol::DaemonResponse {
    let outcome = crate::user_question::route_answer(service, lane_id, call_id, answers, channel);
    if outcome.is_success() {
        return axon_protocol::DaemonResponse::success(request_id, outcome.payload);
    }
    ipc_error_response_with_body(request_id, outcome.payload)
}

/// IPC 失败回执：错误原因统一放在 `error` 字段。
pub(super) fn ipc_error_response(request_id: &str, reason: &str) -> axon_protocol::DaemonResponse {
    ipc_error_response_with_body(request_id, serde_json::json!({ "error": reason }))
}

/// 以给定正文构造 IPC 失败回执。
fn ipc_error_response_with_body(
    request_id: &str,
    body: serde_json::Value,
) -> axon_protocol::DaemonResponse {
    axon_protocol::DaemonResponse {
        request_id: request_id.to_owned(),
        protocol_version: axon_protocol::DAEMON_PROTOCOL_VERSION,
        ok: false,
        body,
        cursor: None,
    }
}
