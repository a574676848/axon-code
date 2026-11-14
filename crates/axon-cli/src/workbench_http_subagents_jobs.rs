//! 工作台子代理与后台作业 HTTP 接口实现。

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use axon_agent_runtime::job_runtime::{JOB_REASON_USER_STOP, JobId, JobRuntime, JobStatus};
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

/// 一次尾随读取的默认字节上限：一屏的体量，超出由游标续读。
pub(super) const JOB_OUTPUT_DEFAULT_MAX_BYTES: usize = 64 * 1024;

/// 一次尾随读取的字节上限：再大的请求按上限收口，避免一次拉走整份归档。
const JOB_OUTPUT_MAX_BYTES: usize = 256 * 1024;

/// 探测归档头部时最多读这么多字节；头部超长时按「没有头部」处理。
const JOB_ARTIFACT_HEADER_PROBE_BYTES: usize = 8 * 1024;

/// 作业状态的稳定小写取值：界面只认这几个 token。
fn job_status_token(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Running => "running",
        JobStatus::Completed => "completed",
        JobStatus::Refusal => "refusal",
        JobStatus::Failed => "failed",
        JobStatus::Killed => "killed",
        JobStatus::Timeout => "timeout",
        JobStatus::Unresponsive => "unresponsive",
    }
}

/// 归档头部的字节数：一直读到 `tty=` 那一行为止。
///
/// 不能按「第三个换行」数：`command=` 里带换行的多行脚本会把头部撑成更多行，
/// 数换行就会漏掉 `cwd=` 与 `tty=`，这两行随即混进正文。认不出 `tty=` 这一行
/// （头部超长或文件刚建）时返回 0，正文从文件起点算。
fn job_artifact_header_bytes(path: &Path) -> u64 {
    let Ok(mut file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut probe = vec![0u8; JOB_ARTIFACT_HEADER_PROBE_BYTES];
    let Ok(read) = file.read(&mut probe) else {
        return 0;
    };
    probe.truncate(read);
    let mut line_start = 0;
    for (index, byte) in probe.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        if probe[line_start..index].starts_with(b"tty=") {
            return u64::try_from(index + 1).unwrap_or(0);
        }
        line_start = index + 1;
    }
    0
}

/// 可安全解码的前缀长度：只在末尾三个字节内回退，因此不会为了一个坏字节
/// 把整段输出反复重校验一遍。
fn utf8_prefix_len(bytes: &[u8]) -> usize {
    if std::str::from_utf8(bytes).is_ok() {
        return bytes.len();
    }
    let floor = bytes.len().saturating_sub(3);
    for end in (floor..bytes.len()).rev() {
        if std::str::from_utf8(&bytes[..end]).is_ok() {
            return end;
        }
    }
    // 非法字节不在末尾：交给有损解码，游标照常前进，不会卡住。
    bytes.len()
}

/// 一次尾随读取最多返回的字节数。
fn bounded_job_output_max(max_bytes: usize) -> usize {
    max_bytes.clamp(1, JOB_OUTPUT_MAX_BYTES)
}

/// 归档里的分片标记行：`----- stdout seq=3 -----`。
///
/// 归档是「命令正文 + 机器分片标记」的混合体：标记用来核对顺序与流别，不是命令
/// 写出的内容，终端卡里不该出现。
fn is_archive_marker(line: &str) -> bool {
    let trimmed = line.trim_end_matches('\r');
    trimmed.starts_with("----- ") && trimmed.ends_with(" -----")
}

/// 去掉分片标记，并把连续空行压成一行。
///
/// 每个分片前后各留了一个空行，原样铺出来会让「最近若干行」里大半是空白，真正要看的
/// 那几行反倒被挤出去。
fn strip_archive_markers(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut previous_blank = false;
    for line in text.lines() {
        if is_archive_marker(line) {
            continue;
        }
        let blank = line.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        previous_blank = blank;
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

/// 读不到时的载荷：作业已不在注册表里，界面据此收口而不是当成空输出。
fn unavailable_job_output(
    lane_id: &str,
    job_id: &str,
    offset: u64,
    reason: &str,
) -> serde_json::Value {
    serde_json::json!({
        "laneId": lane_id,
        "jobId": job_id.trim(),
        "offset": offset,
        "nextOffset": offset,
        "text": "",
        "eof": true,
        "running": false,
        "status": serde_json::Value::Null,
        "exitCode": serde_json::Value::Null,
        "elapsedMs": serde_json::Value::Null,
        "fullOutputAvailable": false,
        "unavailable": reason,
    })
}

/// 某条车道上一次尾随读取作业输出归档。
///
/// 只读归档、**不碰**增量批次：读走增量是模型侧 `job_output` 的语义，界面若也按
/// 增量读，模型本该看到的内容就被吞掉了。游标是归档的字节偏移，调用方按返回的
/// `nextOffset` 续读，因此同一段重复请求既不丢内容也不重复。
pub(super) fn lane_job_output_payload(
    axon_root: &Path,
    lane_id: &str,
    job_id: &str,
    offset: u64,
    max_bytes: usize,
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
    let runtime = JobRuntime::global();
    let Some(owner) = runtime.owner_of(id) else {
        return Ok(unavailable_job_output(
            lane_id,
            job_id,
            offset,
            "作业已结束并超出保留窗口",
        ));
    };
    if !lane_owns_owner(axon_root, lane_id, &owner)? {
        return Err("该作业不属于这条车道".to_string());
    }
    let Some(tail) = runtime.tail(id) else {
        return Ok(unavailable_job_output(
            lane_id,
            job_id,
            offset,
            "作业已结束并超出保留窗口",
        ));
    };
    // 归档头部（`command=` 到 `tty=`）不是作业输出，起点直接越过；判据是 `tty=` 那一行，
    // 而不是行数，因为 `command=` 里可能带换行。只有从头读的那一帧需要定位头部：
    // 后续帧带着已越过头部的游标，再探一次就是每帧白开一个文件句柄读八 KB。
    let header_bytes = if offset == 0 {
        job_artifact_header_bytes(&tail.artifact_path)
    } else {
        0
    };
    let start = offset.max(header_bytes);
    let mut next_offset = start;
    let mut text = String::new();
    let mut eof = true;
    // 归档缺失不是错误：作业可能刚启动、文件尚未落盘，下一帧再读即可。
    if let Ok(metadata) = std::fs::metadata(&tail.artifact_path) {
        let length = metadata.len();
        if start < length {
            let limit = u64::try_from(bounded_job_output_max(max_bytes)).unwrap_or(u64::MAX);
            let want = limit.min(length - start);
            let mut buffer = vec![0u8; usize::try_from(want).unwrap_or(0)];
            let mut file = std::fs::File::open(&tail.artifact_path)
                .map_err(|error| format!("读取作业归档失败：{error}"))?;
            file.seek(SeekFrom::Start(start))
                .map_err(|error| format!("定位作业归档失败：{error}"))?;
            let read = file
                .read(&mut buffer)
                .map_err(|error| format!("读取作业归档失败：{error}"))?;
            buffer.truncate(read);
            let usable = utf8_prefix_len(&buffer);
            // 游标停在整行边界：分片标记行只有完整形态才认得出来，正文也不会被切一半。
            let complete = match buffer[..usable].iter().rposition(|byte| *byte == b'\n') {
                Some(index) => index + 1,
                // 这一帧里一个换行都没有：只有读满上限时才按字节推进，
                // 否则说明这一行还在写，等下一帧再取，免得把长行切成两截。
                None => {
                    if want >= limit {
                        usable
                    } else {
                        0
                    }
                }
            };
            if complete > 0 {
                text = strip_archive_markers(&String::from_utf8_lossy(&buffer[..complete]));
                next_offset = start.saturating_add(u64::try_from(complete).unwrap_or(0));
            }
            eof = next_offset >= length;
        }
    }
    Ok(serde_json::json!({
        "laneId": lane_id,
        "jobId": job_id.trim(),
        "offset": start,
        "nextOffset": next_offset,
        "text": text,
        "eof": eof,
        "running": tail.status == JobStatus::Running,
        "status": job_status_token(tail.status),
        "exitCode": tail.exit_code,
        "elapsedMs": tail.elapsed_ms,
        "fullOutputAvailable": tail.full_output_available,
        "unavailable": serde_json::Value::Null,
    }))
}

#[cfg(test)]
#[path = "workbench_http_subagents_jobs_tests.rs"]
mod workbench_http_subagents_jobs_tests;
