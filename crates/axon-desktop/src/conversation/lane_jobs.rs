//! 一条车道上仍在运行的后台作业名册。
//!
//! 界面里的作业行来自 transcript 投影：它们是**历史事实**，进程重启后不会自己消失。
//! 这份名册给出后端此刻真正持有的作业，界面据此对账：
//! 不在这份名册里的行不再显示为「运行中」，也不能被停止。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::LANES_PATH;
use crate::gateway;

/// 活作业名册。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneJobsRoster {
    /// 目标车道。
    pub lane_id: String,
    /// 后端进程代际；重启即变化，用于识别过期作业行。
    #[serde(default)]
    pub epoch: String,
    /// 仍在运行的作业标识。
    #[serde(default)]
    pub job_ids: Vec<String>,
}

/// 读取某条车道的活作业名册。
pub fn lane_jobs(lane_id: &str) -> Result<LaneJobsRoster, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let payload = gateway::get_json_at(&format!("{LANES_PATH}/{lane_id}/jobs"))?;
    serde_json::from_value::<LaneJobsRoster>(payload)
        .map_err(|error| format!("解析作业名册失败：{error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_lane_id_is_rejected_before_any_request() {
        assert!(super::lane_jobs("   ").is_err());
    }
}
