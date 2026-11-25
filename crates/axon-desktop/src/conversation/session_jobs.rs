//! 一条车道上仍在运行的后台作业名册。
//!
//! 界面里的作业行来自 transcript 投影：它们是**历史事实**，进程重启后不会自己消失。
//! 这份名册给出后端此刻真正持有的作业，界面据此对账：
//! 不在这份名册里的行不再显示为「运行中」，也不能被停止。
//!
//! 同一份响应还回答第二个问题：界面点名的那几条作业**到底跑了多久、怎么收口的**。
//! 两者必须同源：注册表只保活一个很短的窗口，收口事实因此取落盘事件里的实测值，
//! 与会话历史页读的是同一批 `job_started` / `job_completed`。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::SESSIONS_PATH;
use crate::gateway;

/// 活作业名册。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionJobsRoster {
    /// 目标车道。
    pub session_id: String,
    /// 后端进程代际；重启即变化，用于识别过期作业行。
    #[serde(default)]
    pub epoch: String,
    /// 仍在运行的作业标识。
    #[serde(default)]
    pub job_ids: Vec<String>,
    /// 界面点名要的已收口作业事实；没点名时为空。
    #[serde(default)]
    pub settled: Vec<SessionJobFact>,
}

/// 已收口作业的一行事实。
///
/// 与会话历史页的作业清单同源（同一份事件日志、同一套字段名），只取收口这一路径
/// 用得上的四项：`kind` 与 `startedAtMs` 在这一路径没有消费者，网关多下发的键按
/// serde 默认忽略，不影响解析。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionJobFact {
    /// 服务端作业标识；缺失时留空串，界面按「对不上行」忽略它，
    /// 不让一条坏事实把整份名册解析掉。
    #[serde(default)]
    pub job_id: String,
    /// 终态名称：completed / failed / killed / timeout / refusal / unresponsive。
    #[serde(default)]
    pub status: Option<String>,
    /// 从启动到终态的后端实测耗时；未测得时缺省，不用 0 冒充。
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    /// 退出码（若可得）。
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// 读取某条车道的活作业名册，并顺带取回界面点名的收口事实。
///
/// `settled` 只放界面手上有行、名册却已不再持有的那些作业标识：网关据此按需回放
/// 事件日志，台账很长的会话也只读到最后一次收口为止。
pub fn session_jobs(session_id: &str, settled: &[String]) -> Result<SessionJobsRoster, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let payload = gateway::get_json_at(&roster_path(session_id, settled))?;
    serde_json::from_value::<SessionJobsRoster>(payload)
        .map_err(|error| format!("解析作业名册失败：{error}"))
}

/// 名册请求路径：点名了收口事实才带查询串。
///
/// 只放行数字标识：网关那边的作业身份就是自增整数，非数字既不可能是作业，
/// 也不该被拼进查询串。
fn roster_path(session_id: &str, settled: &[String]) -> String {
    let ids = settled
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .collect::<Vec<_>>();
    let base = format!("{SESSIONS_PATH}/{session_id}/jobs");
    if ids.is_empty() {
        return base;
    }
    format!("{base}?settled={}", ids.join(","))
}

#[cfg(test)]
mod tests {
    use super::roster_path;

    #[test]
    fn empty_session_id_is_rejected_before_any_request() {
        assert!(super::session_jobs("   ", &[]).is_err());
    }

    #[test]
    fn roster_path_only_carries_numeric_settled_ids() {
        assert_eq!(
            roster_path("s1", &[]),
            "/api/v1/sessions/s1/jobs",
            "没有点名时不得多带查询串"
        );
        assert_eq!(
            roster_path(
                "s1",
                &[
                    "7".to_owned(),
                    " 8 ".to_owned(),
                    "x".to_owned(),
                    String::new()
                ]
            ),
            "/api/v1/sessions/s1/jobs?settled=7,8",
            "非数字标识既不可能是作业，也不该进查询串"
        );
    }

    #[test]
    fn old_gateway_without_settled_still_parses() {
        // 未升级的网关不认 `?settled=`，响应里也没有该字段：名册照常可用，
        // 只是收口事实为空（界面因此不画耗时，不会拿 0 秒顶替）。
        let payload = serde_json::json!({
            "sessionId": "s1",
            "epoch": "e",
            "jobIds": ["3"],
        });
        let roster: super::SessionJobsRoster =
            serde_json::from_value(payload).expect("旧网关的名册必须可解析");
        assert_eq!(roster.job_ids, vec!["3".to_owned()]);
        assert!(roster.settled.is_empty());
    }
}
