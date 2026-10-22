//! 用户手动停止一条车道上正在运行的后台作业。
//!
//! 走工作台扩展面的作业停止入口：服务端先核对作业归属（主 turn 的作业 owner
//! 就是这条车道的 session lane ID），再把终态原因写成「用户手动停止」。
//! 该原因随作业的完成通知回到发起它的 Agent，模型因此知道是人停的。
//! 归属校验在服务端完成，桌面端不持有第二份判断。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::LANES_PATH;
use crate::gateway;

/// 停止回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStopReceipt {
    /// 目标车道。
    pub lane_id: String,
    /// 被停止的作业。
    pub job_id: String,
    /// 是否确实停止。
    pub stopped: bool,
    /// 终态状态标签。
    #[serde(default)]
    pub status: Option<String>,
    /// 终态原因。
    #[serde(default)]
    pub reason: Option<String>,
}

/// 停止一条车道上的后台作业。
pub fn stop_job(lane_id: &str, job_id: &str) -> Result<JobStopReceipt, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let job_id = job_id.trim();
    if job_id.is_empty() {
        return Err("作业标识不能为空".to_string());
    }
    let path = format!("{LANES_PATH}/{lane_id}/jobs/{job_id}/stop");
    let payload = gateway::post_json_at(&path, &serde_json::json!({}))?;
    serde_json::from_value::<JobStopReceipt>(payload)
        .map_err(|error| format!("解析停止回执失败：{error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_identifiers_are_rejected_before_any_request() {
        assert!(super::stop_job("  ", "1").is_err());
        assert!(super::stop_job("session-lane-a", " ").is_err());
    }
}
