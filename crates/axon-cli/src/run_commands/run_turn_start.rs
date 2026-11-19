//! 一轮对话的开始：起始持久化与 stop 控制通道。
//!
//! 两个 turn 入口共用这一处。共用之外只剩装载端的登记动作（子代理把自己登记进父轮名册、
//! 把会话状态推进到运行中），那不是「一轮怎么开始」，而是「谁开始了一轮」。

use super::{RunRequest, RunSessionRecord, RunStopControl, mirror_session_started};

/// 落一轮的起始态：记录构造与投影只有这一份。
///
/// 返回记录本身：子代理要用同一条记录补它自己的标题投影，而不是再造一条。
pub(crate) fn persist_turn_started(request: &RunRequest) -> Result<RunSessionRecord, String> {
    let record = RunSessionRecord::started(request);
    mirror_session_started(&request.config_path, &record)?;
    Ok(record)
}

/// 起一条 run-control 通道：让 `axon stop` 能停住这一轮。
///
/// 没有取消令牌就无从建立通道。要不要通道、失败是否致命，由调用端按自己的来源决定，
/// 这里只负责“有令牌就建”。
pub(crate) fn start_turn_stop_control(
    request: &RunRequest,
) -> Result<Option<RunStopControl>, String> {
    let Some(cancel_token) = request.cancel_token.clone() else {
        return Ok(None);
    };
    RunStopControl::start(&request.config_path, &request.session_id, cancel_token).map(Some)
}
