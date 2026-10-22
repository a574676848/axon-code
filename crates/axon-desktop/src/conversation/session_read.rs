//! 读取会话级事实：车道的子代理清单，以及任意会话的逐字稿。
//!
//! 子代理是独立会话（自己的逐字稿、模型与工具过程），只是挂在主会话之下。
//! 界面要把它当普通会话打开，就必须能按会话标识读到同一份线程存储，
//! 而不是只拿主轮事件流里的生命周期摘要。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::LANES_PATH;
use crate::gateway;

/// 车道下的一条子代理会话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneSubagentSummary {
    /// 子代理会话标识。
    pub session_id: String,
    /// 会话标识里属于代理路径的那一段，用于与界面上的 agent 路径对齐。
    #[serde(default)]
    pub slug: String,
    /// 会话标题（由首轮任务派生）。
    #[serde(default)]
    pub title: Option<String>,
    /// 该子代理使用的模型。
    #[serde(default)]
    pub model: String,
    /// 该子代理使用的 provider。
    #[serde(default)]
    pub provider: String,
    /// 最近更新时间。
    #[serde(default)]
    pub updated_at_ms: u64,
    /// 线程真实状态：`active` 表示此刻仍在运行。
    ///
    /// 这是「是否还在跑」的唯一可靠依据：主轮事件流里的生命周期事件在轮次结束后
    /// 就不再产生，只靠它会让已结束的子代理永远停在运行中。
    #[serde(default)]
    pub status: String,
    /// 该会话的回合数。
    #[serde(default)]
    pub turn_count: u64,
    /// 末轮状态：结束、中断还是失败由它决定。
    #[serde(default)]
    pub last_turn_status: Option<String>,
    /// 末轮时间。
    #[serde(default)]
    pub last_turn_at_ms: Option<u64>,
    /// 是否有运行态会话行；`false` 表示这次执行早于「子代理会话化」。
    #[serde(default)]
    pub has_runtime: bool,
}

/// 车道子代理清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneSubagents {
    /// 目标车道。
    pub lane_id: String,
    /// 车道运行会话标识；没有运行会话时为空。
    #[serde(default)]
    pub run_session_id: Option<String>,
    /// 该车道派生出的子代理会话。
    #[serde(default)]
    pub subagents: Vec<LaneSubagentSummary>,
}

/// 读取一条车道下的子代理会话清单。
pub fn lane_subagents(lane_id: &str) -> Result<LaneSubagents, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let payload = gateway::get_json_at(&format!("{LANES_PATH}/{lane_id}/subagents"))?;
    serde_json::from_value::<LaneSubagents>(payload)
        .map_err(|error| format!("解析子代理清单失败：{error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_identifiers_are_rejected_before_any_request() {
        assert!(super::lane_subagents("  ").is_err());
    }
}
