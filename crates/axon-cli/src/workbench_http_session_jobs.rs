//! 工作台后台作业的 HTTP 载荷：活作业名册、收口事实、停止与输出增量。
//!
//! 名册与收口事实回答两个不同的问题，且必须同源：
//! - **名册**：后端此刻还持有谁。判据是归属（会话身份与运行窗口）与注册表的未终态集合。
//! - **收口事实**：那一条跑了多久、怎么收口的。来源是落盘事件日志里的 `job_completed`，
//!   与会话历史页读同一份事实；进程内注册表只保活一个很短窗口，不能承担耐久读数。

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use axon_agent_runtime::job_runtime::{JOB_REASON_USER_STOP, JobId, JobRuntime, JobStatus};
use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore, RuntimeStoreError, SqliteRuntimeStore};

use super::runtime_store;
use super::session_readings::{SessionJobFact, collect_job_fact};
use super::workbench_http_targets::session_owns_owner;

/// 名册一次对账最多接受的收口作业标识数。
///
/// 界面只点名它手上那几行，上限只是防住异常请求把一次对账拖成整段日志解压。
pub(super) const JOB_FACT_MAX_IDS: usize = 64;

/// 收口事实的倒序扫描窗口（帧数）。
///
/// 代价与「从尾部走到最近一次收口」的距离成正比，不是与整份日志成正比：命中即停。
/// 界面点名发生在作业刚离开名册的那一拍，收口帧就在尾部附近；取两千帧与会话级事实
/// 扫描的既有窗口（`SESSION_TAIL_MAX_FRAMES`）同量级，一次长流式回答也推不出这个范围。
/// 真到了窗口之外就按「没有实测」呈现，绝不为了一行耗时去扫整条会话。
const JOB_FACT_TAIL_FRAMES: usize = 2_000;

/// 按作业标识取已收口作业的后端实测事实。
///
/// 事实源与会话历史页同一处：事件日志里的 `job_completed`。注册表里的 `tail` 只保活
/// 一个很短的窗口（父轮次结束即释放），而界面要的是「这条作业到底跑了多久」的耐久
/// 事实，因此这里读落盘事件，不读进程内注册表。
fn settled_job_facts(
    store: &SqliteRuntimeStore,
    session: &DaemonSessionId,
    wanted: &[String],
) -> Vec<SessionJobFact> {
    // 上限在取数这一层也兜一次：HTTP 入口已按 JOB_FACT_MAX_IDS 截断，
    // 但这条路径也可能被别的调用方直接调，扫描代价不能交给调用方自律。
    let wanted: std::collections::HashSet<&str> = wanted
        .iter()
        .take(JOB_FACT_MAX_IDS)
        .map(String::as_str)
        .filter(|id| !id.is_empty())
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    // 复用本次请求已经开着的运行态连接：为一次点名再开一条连接只是白付一次开销。
    let path = store.event_log(session).path();
    if !path.exists() {
        return Vec::new();
    }
    let mut facts: Vec<SessionJobFact> = Vec::new();
    let mut pending = wanted.len();
    // 倒序扫描：同一个作业取最近一次收口，早先的重试记录不再覆盖。
    let _ = axon_eventlog::read_tail_scan(&path, JOB_FACT_TAIL_FRAMES, |record| {
        if record.event_type.as_str() != "job_completed" {
            return true;
        }
        let Some(job_id) = record
            .payload
            .get("jobId")
            .and_then(serde_json::Value::as_str)
        else {
            return true;
        };
        if !wanted.contains(job_id) || facts.iter().any(|fact| fact.job_id == job_id) {
            return true;
        }
        collect_job_fact(record, &mut facts);
        pending = pending.saturating_sub(1);
        pending > 0
    });
    facts
}

/// 某条车道上仍在运行的后台作业名册。
///
/// `settled_ids` 是界面点名要收口事实的那些作业（它手上有行、但名册已不再持有的）。
/// 名册本身只回答「还持有谁」，收口事实另给一份，两者因此不会互相冒充。
pub(super) fn session_jobs_payload(
    axon_root: &Path,
    session_id: &str,
    settled_ids: &[String],
) -> Result<serde_json::Value, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let store = runtime_store(axon_root);
    let session_id =
        DaemonSessionId::new(session_id).map_err(|error| format!("车道标识非法：{error:?}"))?;
    let (job_ids, settled) = match store.get_session(&session_id) {
        /*
         * 归属判据必须与停止入口同源（`owner_matches_session`）：Shell 工具把会话身份
         * 写进作业的 `owner_id`，运行窗口只是同一会话的换窗标识。只按窗口查会让名册
         * 恒空，界面据此把仍在跑的作业判成「已结束」，停止键与归档确认一起失效。
         */
        Ok(record) => {
            let runtime = JobRuntime::global();
            let mut ids = runtime.pending_job_ids(session_id.as_str());
            ids.extend(runtime.pending_job_ids(&record.current_run_id));
            ids.sort_unstable();
            ids.dedup();
            let job_ids = ids
                .into_iter()
                .map(|id| id.0.to_string())
                .collect::<Vec<_>>();
            // 还在名册里的作业不谈收口：它在跑的耗时由界面按秒走。
            let asked = settled_ids
                .iter()
                .filter(|id| !job_ids.contains(id))
                .cloned()
                .collect::<Vec<_>>();
            let settled = settled_job_facts(&store, &session_id, &asked);
            (job_ids, settled)
        }
        Err(RuntimeStoreError::NotFound(_)) => (Vec::new(), Vec::new()),
        Err(error) => return Err(format!("读取车道运行会话失败：{error:?}")),
    };
    Ok(serde_json::json!({
        "sessionId": session_id,
        "epoch": axon_agent_runtime::job_runtime::runtime_epoch(),
        "jobIds": job_ids,
        "settled": settled,
    }))
}

/// 停止某条车道上正在运行的后台作业。
pub(super) fn stop_session_job(
    axon_root: &Path,
    session_id: &str,
    job_id: &str,
) -> Result<serde_json::Value, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
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
    if !session_owns_owner(axon_root, session_id, &owner)? {
        return Err("该作业不属于这条车道".to_string());
    }
    let status = JobRuntime::global()
        .kill_with_reason(id, &owner, Some(JOB_REASON_USER_STOP))
        .map_err(|error| format!("停止作业失败：{error:?}"))?;
    Ok(serde_json::json!({
        "sessionId": session_id,
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
    session_id: &str,
    job_id: &str,
    offset: u64,
    reason: &str,
) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id,
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
pub(super) fn session_job_output_payload(
    axon_root: &Path,
    session_id: &str,
    job_id: &str,
    offset: u64,
    max_bytes: usize,
) -> Result<serde_json::Value, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
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
            session_id,
            job_id,
            offset,
            "作业已结束并超出保留窗口",
        ));
    };
    if !session_owns_owner(axon_root, session_id, &owner)? {
        return Err("该作业不属于这条车道".to_string());
    }
    let Some(tail) = runtime.tail(id) else {
        return Ok(unavailable_job_output(
            session_id,
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
        "sessionId": session_id,
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
#[path = "workbench_http_session_jobs_tests.rs"]
mod workbench_http_session_jobs_tests;
