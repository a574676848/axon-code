//! 中断正在运行的主 turn。
//!
//! 走工作台扩展面的车道中断入口：服务端把该车道活跃主 turn 的取消令牌置位，
//! provider 传输、重试退避与工具执行的所有取消点随即收口，本轮按「已中断」结束。
//! 取消令牌是后端既有机制，桌面端只负责把请求送达，不做任何本地状态假设。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::LANES_PATH;
use crate::gateway;

/// 中断回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptReceipt {
    /// 目标车道。
    pub lane_id: String,
    /// 是否确实中断了一轮。
    pub interrupted: bool,
    /// 被中断的 Interaction 标识；没有活跃 turn 时为空。
    #[serde(default)]
    pub interaction_id: Option<String>,
}

/// 中断一条车道上正在运行的主 turn。
pub fn interrupt_lane(lane_id: &str) -> Result<InterruptReceipt, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let path = format!("{LANES_PATH}/{lane_id}/interrupt");
    let payload = gateway::post_json_at(&path, &serde_json::json!({}))?;
    serde_json::from_value::<InterruptReceipt>(payload)
        .map_err(|error| format!("解析中断回执失败：{error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_lane_id_is_rejected_before_any_request() {
        assert!(super::interrupt_lane("  ").is_err());
    }
}
