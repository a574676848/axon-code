//! 工作台子代理与后台作业 HTTP 接口实现。

use std::path::Path;

use axon_agent_runtime::job_runtime::{JOB_REASON_USER_STOP, JobId, JobRuntime};
use axon_domain::DaemonSessionId;
use axon_session::{JsonlThreadEventStore, RuntimeStore, RuntimeStoreError, ThreadService};

use super::runtime_store;
use super::workbench_http_targets::lane_owns_owner;

/// 子代理会话 ID 的分隔符，与运行侧 `child_session_id` 保持一致。
const SUBAGENT_SESSION_ID_SEPARATOR: &str = "--subagent--";
/// 一次车道子代理列举的上限。
const MAX_LANE_SUBAGENTS: usize = 200;

/// 会话数据源：与 `axon export` 同一份线程存储。
fn thread_service() -> ThreadService<JsonlThreadEventStore> {
    ThreadService::new(JsonlThreadEventStore)
}

/// 车道的运行会话 ID；子代理会话挂在它下面。
fn lane_run_session_id(axon_root: &Path, lane_id: &str) -> Result<String, String> {
    let store = runtime_store(axon_root);
    let session_id =
        DaemonSessionId::new(lane_id).map_err(|error| format!("车道标识非法：{error:?}"))?;
    match store.get_session(&session_id) {
        Ok(record) => Ok(record.run_session_id),
        Err(RuntimeStoreError::NotFound(_)) => Ok(String::new()),
        Err(error) => Err(format!("读取车道运行会话失败：{error:?}")),
    }
}

/// 线程状态的稳定小写取值：界面只认这几个 token。
fn thread_status_token(status: axon_session::ThreadStatus) -> &'static str {
    match status {
        axon_session::ThreadStatus::NotLoaded => "notLoaded",
        axon_session::ThreadStatus::Idle => "idle",
        axon_session::ThreadStatus::Active => "active",
        axon_session::ThreadStatus::SystemError => "systemError",
        axon_session::ThreadStatus::Archived => "archived",
    }
}

/// 末轮状态的稳定小写取值；没有回合时为空。
fn last_turn_status_token(detail: &axon_session::ThreadDetail) -> Option<&'static str> {
    let turn = detail.turns.last()?;
    Some(match turn.status {
        axon_session::TurnStatus::Active => "active",
        axon_session::TurnStatus::Completed => "completed",
        axon_session::TurnStatus::Interrupted => "interrupted",
        axon_session::TurnStatus::Failed => "failed",
    })
}

/// 某条车道下的子代理会话清单。
pub(super) fn lane_subagents_payload(
    axon_root: &Path,
    lane_id: &str,
) -> Result<serde_json::Value, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let run_session_id = lane_run_session_id(axon_root, lane_id)?;
    if run_session_id.is_empty() {
        return Ok(serde_json::json!({
            "laneId": lane_id,
            "runSessionId": serde_json::Value::Null,
            "subagents": [],
        }));
    }
    let prefix = format!("{run_session_id}{SUBAGENT_SESSION_ID_SEPARATOR}");
    let service = thread_service();
    let threads = service
        .list_threads(axon_root, MAX_LANE_SUBAGENTS)
        .map_err(|error| format!("读取子代理会话失败：{error}"))?;
    let children = threads
        .into_iter()
        .filter(|summary| summary.thread_id.as_str().starts_with(&prefix))
        .collect::<Vec<_>>();
    let mut subagents = Vec::with_capacity(children.len());
    for summary in children {
        let session_id = summary.thread_id.as_str().to_owned();
        let slug = session_id
            .strip_prefix(&prefix)
            .unwrap_or_default()
            .to_owned();
        let (last_turn_status, last_turn_at_ms) = match service.read_thread(axon_root, &session_id)
        {
            Ok(detail) => (
                last_turn_status_token(&detail),
                detail
                    .turns
                    .last()
                    .map(|turn| turn.completed_at_ms.unwrap_or(turn.started_at_ms)),
            ),
            Err(_) => (None, None),
        };
        let has_runtime = DaemonSessionId::new(session_id.as_str())
            .ok()
            .is_some_and(|id| runtime_store(axon_root).get_session(&id).is_ok());
        subagents.push(serde_json::json!({
            "sessionId": session_id,
            "slug": slug,
            "title": summary.name,
            "model": summary.model,
            "provider": summary.provider,
            "updatedAtMs": summary.updated_at_ms,
            "hasRuntime": has_runtime,
            "status": thread_status_token(summary.status),
            "turnCount": summary.turn_count,
            "lastTurnStatus": last_turn_status,
            "lastTurnAtMs": last_turn_at_ms,
        }));
    }
    Ok(serde_json::json!({
        "laneId": lane_id,
        "runSessionId": run_session_id,
        "subagents": subagents,
    }))
}

/// 某条车道上仍在运行的后台作业名册。
pub(super) fn lane_jobs_payload(
    axon_root: &Path,
    lane_id: &str,
) -> Result<serde_json::Value, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let store = runtime_store(axon_root);
    let session_id =
        DaemonSessionId::new(lane_id).map_err(|error| format!("车道标识非法：{error:?}"))?;
    let job_ids = match store.get_session(&session_id) {
        Ok(record) => JobRuntime::global()
            .pending_job_ids(&record.run_session_id)
            .into_iter()
            .map(|id| id.0.to_string())
            .collect::<Vec<_>>(),
        Err(RuntimeStoreError::NotFound(_)) => Vec::new(),
        Err(error) => return Err(format!("读取车道运行会话失败：{error:?}")),
    };
    Ok(serde_json::json!({
        "laneId": lane_id,
        "epoch": axon_agent_runtime::job_runtime::runtime_epoch(),
        "jobIds": job_ids,
    }))
}

/// 停止某条车道上正在运行的后台作业。
pub(super) fn stop_lane_job(
    axon_root: &Path,
    lane_id: &str,
    job_id: &str,
) -> Result<serde_json::Value, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let parsed = job_id
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("作业标识非法：{job_id}"))?;
    let id = JobId(parsed);
    let owner = JobRuntime::global()
        .owner_of(id)
        .ok_or_else(|| "作业不存在或已过保留窗口".to_string())?;
    if !lane_owns_owner(axon_root, lane_id, &owner)? {
        return Err("该作业不属于这条车道".to_string());
    }
    let status = JobRuntime::global()
        .kill_with_reason(id, &owner, Some(JOB_REASON_USER_STOP))
        .map_err(|error| format!("停止作业失败：{error:?}"))?;
    Ok(serde_json::json!({
        "laneId": lane_id,
        "jobId": job_id.trim(),
        "stopped": true,
        "status": format!("{status:?}"),
        "reason": JOB_REASON_USER_STOP,
    }))
}
