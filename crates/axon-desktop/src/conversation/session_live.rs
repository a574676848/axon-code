//! 会话运行态证据读取。
//!
//! 「在跑」必须能指回带租约且租约未过期的行：界面自己按投影推算出来的在跑态，
//! 在进程消失或租约过期之后不会自己消失。本模块只读，不做任何本地状态改写。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::SESSIONS_PATH;
use crate::gateway;

/// 一条会话的运行态证据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLive {
    /// 目标会话。
    pub session_id: String,
    /// 是否存在带租约且租约未过期的在跑轮次。
    pub running: bool,
    /// 未过期租约的剩余时长（毫秒）；没有在跑轮次时为空。
    #[serde(default)]
    pub lease_remaining_ms: Option<u64>,
    /// 该会话 durable 事件尾序号。
    #[serde(default)]
    pub last_frame_sequence: u64,
    /// 距离最后一次活动的静默时长（毫秒）。
    #[serde(default)]
    pub idle_for_ms: u64,
}

/// 读取一条会话的运行态证据。
pub fn read_session_live(session_id: &str) -> Result<SessionLive, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("会话标识不能为空".to_string());
    }
    let payload = gateway::get_json_at(&format!("{SESSIONS_PATH}/{session_id}/live"))?;
    serde_json::from_value::<SessionLive>(payload)
        .map_err(|error| format!("解析运行态证据失败：{error}"))
}

#[cfg(test)]
mod tests {
    /// 空标识在发请求之前就被拒绝。
    #[test]
    fn empty_session_id_is_rejected_before_any_request() {
        assert!(super::read_session_live("  ").is_err());
    }

    /// 没有租约余量时读成未知，而不是 0：读侧据此区分「没有在跑」与「余量归零」。
    #[test]
    fn payload_keeps_null_lease_remaining() {
        let payload = serde_json::json!({
            "sessionId": "session-1",
            "running": false,
            "leaseRemainingMs": serde_json::Value::Null,
            "lastFrameSequence": 6230,
            "idleForMs": 3400,
        });
        let live = serde_json::from_value::<super::SessionLive>(payload).expect("证据应可解析");
        assert!(!live.running);
        assert_eq!(live.lease_remaining_ms, None);
        assert_eq!(live.last_frame_sequence, 6230);
        assert_eq!(live.idle_for_ms, 3400);
    }
}
