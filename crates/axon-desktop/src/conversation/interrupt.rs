//! 中断正在运行的主 turn。
//!
//! 走工作台扩展面的车道中断入口：服务端把该车道活跃主 turn 的取消令牌置位，
//! provider 传输、重试退避与工具执行的所有取消点随即收口，本轮按「已中断」结束。
//! 取消令牌是后端既有机制，桌面端只负责把请求送达，不做任何本地状态假设。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::SESSIONS_PATH;
use crate::gateway;

/// 中断回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptReceipt {
    /// 目标车道。
    pub session_id: String,
    /// 是否确实中断了一轮。
    pub interrupted: bool,
    /// 取消是否已受理：为真表示本轮会在下一个安全检查点收口，而不是已经停下。
    #[serde(default)]
    pub cancellation_requested: bool,
    /// 被中断的 Interaction 标识；没有活跃 turn 时为空。
    #[serde(default)]
    pub interaction_id: Option<String>,
}

/// 显式收口回执：服务端是否真的收口，以及它给出的判据。
///
/// 判据取值：`lease_expired`（租约已过期）、`first_token_deadline_exceeded`（首字超时）、
/// `still_running`（仍在跑，不收口）、`no_in_flight_turn`（没有在途轮次）、
/// `already_settled`（之前已经收口）。界面按判据给文案，不自己推断结论。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleSilentTurnReceipt {
    /// 本次是否真的收口了这一轮。
    pub settled: bool,
    /// 服务端判据。
    pub verdict: String,
}

/// 中断一条车道上正在运行的主 turn。
pub fn interrupt_session(session_id: &str) -> Result<InterruptReceipt, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let path = format!("{SESSIONS_PATH}/{session_id}/interrupt");
    let payload = gateway::post_json_at(&path, &serde_json::json!({}))?;
    serde_json::from_value::<InterruptReceipt>(payload)
        .map_err(|error| format!("解析中断回执失败：{error}"))
}

/// 请服务端显式收口一条静默轮次。
///
/// 索引仍说在途、服务端却没有运行轮次时，界面不能自己把这一轮标成已中断：
/// 收口是服务端的事实，这里只负责把请求送达，并按回执里的判据给文案。
pub fn settle_silent_turn(
    session_id: &str,
    interaction_id: &str,
) -> Result<SettleSilentTurnReceipt, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let interaction_id = interaction_id.trim();
    if interaction_id.is_empty() {
        return Err("轮次标识不能为空".to_string());
    }
    let path = format!("{SESSIONS_PATH}/{session_id}/settle-silent-turn");
    let payload = gateway::post_json_at(
        &path,
        &serde_json::json!({ "interactionId": interaction_id }),
    )?;
    serde_json::from_value::<SettleSilentTurnReceipt>(payload)
        .map_err(|error| format!("解析收口回执失败：{error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_session_id_is_rejected_before_any_request() {
        assert!(super::interrupt_session("  ").is_err());
    }

    /// 收口请求的两个标识都必须在发请求之前校验：缺一个就是畸形请求。
    #[test]
    fn settle_requires_both_identifiers() {
        assert!(super::settle_silent_turn("  ", "interaction-1").is_err());
        assert!(super::settle_silent_turn("session-1", "  ").is_err());
    }
}
