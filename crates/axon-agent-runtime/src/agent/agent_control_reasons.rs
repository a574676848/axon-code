//! 子代理控制面的稳定拒绝原因码。  use super::*;
/// 子代理路径已存在。
pub const AGENT_CONTROL_REASON_PATH_EXISTS: &str = "agent_control:path_exists";
/// 子代理路径无效。
pub const AGENT_CONTROL_REASON_INVALID_PATH: &str = "agent_control:invalid_path";
/// 子代理未找到。
pub const AGENT_CONTROL_REASON_NOT_FOUND: &str = "agent_control:not_found";
/// 子代理数量达到上限。
pub const AGENT_CONTROL_REASON_LIMIT_REACHED: &str = "agent_control:limit_reached";
/// 子代理深度达到上限。
pub const AGENT_CONTROL_REASON_DEPTH_LIMIT_REACHED: &str = "agent_control:depth_limit_reached";
/// 子代理昵称冲突。
pub const AGENT_CONTROL_REASON_NICKNAME_EXISTS: &str = "agent_control:nickname_exists";
/// 预留信息缺失。
pub const AGENT_CONTROL_REASON_RESERVATION_MISSING: &str = "agent_control:reservation_missing";
/// 子代理消息内容为空。
pub const AGENT_CONTROL_REASON_EMPTY_MESSAGE: &str = "agent_control:empty_message";
/// 接收方暂时不可接收消息（已终态或已关闭）。
///
/// 与 [`AGENT_CONTROL_REASON_NOT_FOUND`] 区分：不存在是非法目标（稳定拒绝、不落盘），
/// 暂时不可接收是合法目标但当下无法交付，消息落为待投递并由重试推进（合同 §5.3.2）。
pub const AGENT_CONTROL_REASON_RECIPIENT_UNAVAILABLE: &str = "agent_control:recipient_unavailable";
