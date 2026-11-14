//! 后台作业输出的尾随读取。
//!
//! 作业的真实输出落在运行侧的归档文件里，网关按**字节游标**增量返回。这里只做
//! 参数校验与载荷解析，不决定轮询节奏：节奏属于界面（见
//! `ui/src/components/conversation/useJobStream.ts`），换一种节奏不该改后端契约。
//!
//! 与 `lane_jobs` 的分工：名册回答「后端此刻还持有哪些作业」，这里回答「某个作业
//! 到第几个字节为止输出了什么」。两者都由网关给出，桌面端不直连运行态数据。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::LANES_PATH;
use crate::gateway;

/// 一次尾随读取的最大字节数：与网关默认口径一致，请求里显式带上以免依赖缺省值。
const TAIL_PAGE_BYTES: u32 = 64 * 1024;

/// 作业输出的一页。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobOutputStream {
    /// 目标车道。
    pub lane_id: String,
    /// 目标作业。
    pub job_id: String,
    /// 本次读取的起始字节偏移（已在归档头部之后）。
    pub offset: u64,
    /// 下一次读取的字节偏移。
    pub next_offset: u64,
    /// 本次读到的输出正文。
    #[serde(default)]
    pub text: String,
    /// 是否已读到归档末尾。作业在跑时它也会为真，是否继续跟随看 [`Self::running`]。
    #[serde(default)]
    pub eof: bool,
    /// 作业是否仍在运行。
    #[serde(default)]
    pub running: bool,
    /// 作业状态；作业已被回收时为空。
    #[serde(default)]
    pub status: Option<String>,
    /// 终态退出码。
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// 已运行毫秒数。
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    /// 归档是否完整（未被容量上限截断、写入未失败）。
    #[serde(default)]
    pub full_output_available: bool,
    /// 读不到的原因；为空表示这一页就是真实输出。
    #[serde(default)]
    pub unavailable: Option<String>,
}

/// 读取某条车道上一次作业输出增量。
pub fn job_output(lane_id: &str, job_id: &str, offset: u64) -> Result<JobOutputStream, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let job_id = job_id.trim();
    if job_id.is_empty() {
        return Err("作业标识不能为空".to_string());
    }
    let payload = gateway::get_json_at(&format!(
        "{LANES_PATH}/{lane_id}/jobs/{job_id}/output?offset={offset}&max={TAIL_PAGE_BYTES}"
    ))?;
    serde_json::from_value::<JobOutputStream>(payload)
        .map_err(|error| format!("解析作业输出失败：{error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_identifiers_are_rejected_before_any_request() {
        assert!(super::job_output("   ", "1", 0).is_err());
        assert!(super::job_output("session-lane-a", "  ", 0).is_err());
    }
}
