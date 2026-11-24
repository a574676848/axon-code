//! 会话事件正文归档：旧交互的大载荷帧正文搬到归档文件，日志里只留引用。
//!
//! 为什么需要它：事件日志是 append-only 的 zstd 帧串接，长会话里历史正文（流式增量、
//! 工具结果、终端输出）占掉了绝大部分字节，而翻页读、备份、恢复都要按会话整体付代价。
//! 索引页要的答案、工具名与读数已经落在 `turn_activity` 投影里，因此旧交互的正文可以
//! 离开日志、留一份可按需回读的归档。
//!
//! 三条边界：
//!
//! - **只搬正文，不动序号**：归档替换的是帧的 `event_type` 与 `payload`，记录条数与序号
//!   原样保留，读侧的跨帧连续性校验与分级前完全一致；
//! - **归档可缺失**：归档目录随会话回收，文件缺失时调用方降级成
//!   `archive_missing` 占位，绝不把「正文已回收」伪装成「这一轮没有正文」；
//! - **路径无关**：归档引用是相对归档根的路径，不含机器相关的绝对路径。

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use axon_eventlog::{EventLog, RuntimeEventKind, RuntimeEventRecord};

use crate::ports::RuntimeStoreError;

/// 被分级事件的正文标记类型：日志里只留这一条轻量标记。
pub const SESSION_EVENT_BODY_ARCHIVED_TYPE: &str = "interaction_body_archived";
/// 归档文件后缀：单帧 zstd 压缩的 JSONL。
///
/// 与事件日志本身同一套压缩（level 3、自带校验和）。归档不压缩等于把已经压过的正文
/// 以明文再存一遍：日志确实变小了，磁盘总量反而变大，"分级回收"就成了账面游戏。
const BODY_ARCHIVE_SUFFIX: &str = ".jsonl.zst";
/// 归档临时文件后缀。
const BODY_ARCHIVE_TMP_SUFFIX: &str = ".tmp";
/// 流式帧的线上事件类型：它承载对话正文的绝大部分字节。
const WIRE_EVENT_TYPE: &str = "wire_event";

/// 归档根下的文件路径。
///
/// 会话与交互标识都按事件目录同一套转义规则编码：两者都可能来自外部渠道，不能直接
/// 拼进路径。
#[must_use]
pub fn body_archive_path(archive_root: &Path, session_id: &str, interaction_id: &str) -> PathBuf {
    archive_root
        .join(EventLog::encode_segment(session_id))
        .join(format!(
            "{}{BODY_ARCHIVE_SUFFIX}",
            EventLog::encode_segment(interaction_id)
        ))
}

/// 归档引用文本：相对归档根，不含盘符与用户目录。
#[must_use]
pub fn body_archive_ref(session_id: &str, interaction_id: &str) -> String {
    format!(
        "{}/{}{BODY_ARCHIVE_SUFFIX}",
        EventLog::encode_segment(session_id),
        EventLog::encode_segment(interaction_id)
    )
}

/// 一条记录是否属于「正文」：只有大载荷的内容帧才值得外置。
///
/// 只认四类内容帧：流式增量（`wire_event`）、工具调用、工具结果与 assistant 正文。
/// 指标、终态、归属状态这些帧一律留在日志里：它们是索引页与回放判定的事实。
#[must_use]
pub fn is_body_record(record: &RuntimeEventRecord, inline_limit_bytes: usize) -> bool {
    let content_frame = matches!(
        record.event_type,
        RuntimeEventKind::ToolCall
            | RuntimeEventKind::ToolResult
            | RuntimeEventKind::AssistantMessage
    ) || record.event_type.as_str() == WIRE_EVENT_TYPE;
    content_frame && payload_bytes(record) >= inline_limit_bytes
}

/// 载荷序列化后的字节数：判定阈值与归档记账共用同一口径。
#[must_use]
pub fn payload_bytes(record: &RuntimeEventRecord) -> usize {
    serde_json::to_vec(&record.payload).map_or(0, |bytes| bytes.len())
}

/// 生成一条正文标记记录：保留序号、归属、代际与时刻，只换类型与载荷。
#[must_use]
pub fn body_marker_record(record: &RuntimeEventRecord, archive_ref: &str) -> RuntimeEventRecord {
    let mut marker = record.clone();
    marker.event_type = RuntimeEventKind::Other(SESSION_EVENT_BODY_ARCHIVED_TYPE.to_owned());
    marker.payload = serde_json::json!({
        "archiveRef": archive_ref,
        "bytes": payload_bytes(record),
    });
    marker
}

/// 是否正文标记记录。
#[must_use]
pub fn is_body_marker(record: &RuntimeEventRecord) -> bool {
    record.event_type.as_str() == SESSION_EVENT_BODY_ARCHIVED_TYPE
}

/// 写入一条交互的正文归档，返回落盘字节数（压缩后）。
///
/// 先写同目录临时文件再原子替换：中途失败时不会留下半份归档被读侧当成完整正文。
pub fn write_body_archive(
    archive_root: &Path,
    session_id: &str,
    interaction_id: &str,
    records: &[RuntimeEventRecord],
) -> Result<u64, RuntimeStoreError> {
    let path = body_archive_path(archive_root, session_id, interaction_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            RuntimeStoreError::Storage(format!("创建正文归档目录失败：{error}"))
        })?;
    }
    let mut plain = Vec::new();
    for record in records {
        let line = record.encode_line().map_err(|error| {
            RuntimeStoreError::Storage(format!("序列化正文归档记录失败：{error}"))
        })?;
        plain.extend_from_slice(&line);
        plain.push(b'\n');
    }
    let body = axon_eventlog::encode_frame(&plain)
        .map_err(|error| RuntimeStoreError::Storage(format!("压缩正文归档失败：{error}")))?;
    let temporary = temporary_path(&path);
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&body)?;
        file.sync_data()
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(RuntimeStoreError::Storage(format!(
            "写入正文归档失败：{error}"
        )));
    }
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(RuntimeStoreError::Storage(format!(
            "替换正文归档失败：{error}"
        )));
    }
    Ok(u64::try_from(body.len()).unwrap_or(u64::MAX))
}

/// 读回一条交互的正文归档；文件不存在（已被回收）时返回 `None`。
pub fn read_body_archive(
    archive_root: &Path,
    session_id: &str,
    interaction_id: &str,
) -> Result<Option<Vec<RuntimeEventRecord>>, RuntimeStoreError> {
    let path = body_archive_path(archive_root, session_id, interaction_id);
    let body = match std::fs::read(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RuntimeStoreError::Storage(format!(
                "读取正文归档失败：{error}"
            )));
        }
    };
    let plain = axon_eventlog::decode_frame(&body)
        .map_err(|error| RuntimeStoreError::Storage(format!("正文归档解压失败：{error}")))?;
    let mut records = Vec::new();
    for line in plain.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let record = RuntimeEventRecord::decode_line(line).map_err(|error| {
            RuntimeStoreError::Storage(format!("正文归档记录无法解析：{error}"))
        })?;
        records.push(record);
    }
    Ok(Some(records))
}

/// 按序号索引归档记录：回读时逐条替换标记，同一序号就是同一条原记录。
#[must_use]
pub fn by_sequence(records: Vec<RuntimeEventRecord>) -> HashMap<u64, RuntimeEventRecord> {
    records
        .into_iter()
        .map(|record| (record.sequence, record))
        .collect()
}

/// 归档文件缺失或被回收时的占位记录。
///
/// 保留原序号与归属，只把载荷换成回收事实；网关据此跳过并留痕，界面因此不会把
/// 「正文已回收」误读成「这一轮本来就没有正文」。
#[must_use]
pub fn reclaimed_placeholder(
    record: &RuntimeEventRecord,
    archive_ref: &str,
    reason: &str,
) -> RuntimeEventRecord {
    let mut placeholder = record.clone();
    placeholder.event_type = RuntimeEventKind::Other(
        crate::infrastructure::SESSION_EVENT_MISSING_ARCHIVE_TYPE.to_owned(),
    );
    placeholder.payload = serde_json::json!({
        "archiveRef": archive_ref,
        "reason": reason,
    });
    placeholder
}

/// 归档临时文件路径：与归档同目录，保证替换不跨卷。
fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(BODY_ARCHIVE_TMP_SUFFIX);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sequence: u64, interaction: &str, text: &str) -> RuntimeEventRecord {
        RuntimeEventRecord::new(
            sequence,
            sequence,
            RuntimeEventKind::Other("wire_event".to_owned()),
            serde_json::json!({ "interactionId": interaction, "text": text }),
        )
        .with_interaction(interaction)
        .with_generation("generation-1")
    }

    #[test]
    fn archive_round_trips_without_losing_identity_fields() {
        let directory = tempfile::tempdir().expect("临时目录");
        let root = directory.path();
        let original = vec![
            record(7, "interaction-a", "正文"),
            record(8, "interaction-a", "续"),
        ];

        let written = write_body_archive(root, "session-a", "interaction-a", &original).unwrap();
        assert!(written > 0);
        let read = read_body_archive(root, "session-a", "interaction-a")
            .unwrap()
            .expect("归档应存在");
        let indexed = by_sequence(read);
        assert_eq!(indexed.get(&7).unwrap().payload["text"], "正文");
        assert_eq!(indexed[&8].generation.as_deref(), Some("generation-1"));
    }

    #[test]
    fn missing_archive_is_reported_as_missing_not_empty() {
        let directory = tempfile::tempdir().expect("临时目录");
        assert!(
            read_body_archive(directory.path(), "session-a", "interaction-a")
                .unwrap()
                .is_none(),
            "归档缺失必须如实返回 None，不能伪装成空正文"
        );
    }

    #[test]
    fn marker_keeps_sequence_and_drops_the_body() {
        let original = record(9, "interaction-a", &"x".repeat(8_192));
        let marker = body_marker_record(&original, "session-a/interaction-a.jsonl");
        assert!(is_body_marker(&marker));
        assert_eq!(marker.sequence, 9);
        assert_eq!(marker.interaction_id.as_deref(), Some("interaction-a"));
        assert!(
            payload_bytes(&marker) < 200,
            "标记必须远小于正文，实际 {} 字节",
            payload_bytes(&marker)
        );
        assert_eq!(marker.payload["bytes"], payload_bytes(&original));
    }

    #[test]
    fn body_records_are_only_large_content_frames() {
        let large = record(1, "interaction-a", &"x".repeat(8_192));
        assert!(is_body_record(&large, 4 * 1024));
        let small = record(2, "interaction-a", "ok");
        assert!(!is_body_record(&small, 4 * 1024), "小体量正文留在日志里");
        let mut metrics = record(3, "interaction-a", &"x".repeat(8_192));
        metrics.event_type = RuntimeEventKind::TurnMetrics;
        assert!(!is_body_record(&metrics, 4 * 1024), "指标帧不得被外置");
    }

    #[test]
    fn archive_paths_never_escape_the_root() {
        let path = body_archive_path(
            Path::new("/archive/session-events"),
            "session/../a",
            "interaction\\b",
        );
        let text = path.to_string_lossy().replace('\\', "/");
        assert!(!text.contains(".."), "路径不得含上跳：{text}");
        assert_eq!(
            text.matches('/').count(),
            4,
            "只允许 archive/session-events/<session>/<interaction>：{text}"
        );
    }
}
