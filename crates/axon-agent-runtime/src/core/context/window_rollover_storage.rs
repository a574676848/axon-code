//! 窗口归档快照的存储、恢复与逆序流式扫描。

use std::path::Path;

use axon_domain::{AxonError, AxonResult};
use axon_jsonl::ShardedJsonlStore;

use crate::window_rollover::{
    ARCHIVED_HISTORY_DIR_NAME, ARCHIVED_WINDOW_SCHEMA_VERSION, ArchivedWindowRecord,
    archive_checksum, archive_shard_file_name,
};

/// 窗口归档使用的统一物理 shard 存储；窗口号是业务层逻辑分片。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

/// 逆序扫描的诊断结果。
#[derive(Debug, Default)]
pub(crate) struct ArchiveReverseScanReport {
    pub(crate) corrupted_records: usize,
    pub(crate) partial: bool,
    pub(crate) next_cursor: Option<String>,
}

pub(crate) fn restore_archive_snapshot(
    session_dir: &Path,
    window_number: u64,
    previous_archive: Option<&[u8]>,
) -> AxonResult<()> {
    let logical_path = archive_logical_path(session_dir, window_number);
    match previous_archive {
        Some(bytes) => {
            let lines = bytes
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .map(Vec::from)
                .collect();
            SHARD_STORE
                .rewrite_with(logical_path, |_| Ok(((), lines)))
                .map_err(|error| AxonError::runtime_failure(format!("恢复归档快照失败: {error:?}")))
        }
        None => SHARD_STORE
            .remove_all(logical_path)
            .map(|_: bool| ())
            .map_err(|error| {
                AxonError::runtime_failure(format!("删除本次归档分片失败: {error:?}"))
            }),
    }
}

/// 读取指定窗口的已提交记录，并重建可用于回滚的物理 JSONL 快照。
pub(crate) fn read_archive_snapshot(
    session_dir: &Path,
    window_number: u64,
) -> AxonResult<Option<Vec<u8>>> {
    let records = read_archive_records(session_dir, window_number)?;
    if records.is_empty() {
        return Ok(None);
    }
    encode_records(&records).map(Some)
}

pub(crate) fn append_archive_records(
    session_dir: &Path,
    records: &[ArchivedWindowRecord],
) -> AxonResult<()> {
    if records.is_empty() {
        return Ok(());
    }
    validate_record_batch(records)?;
    let logical_path = archive_logical_path(session_dir, records[0].window_number);
    let payload = encode_records(records)?;
    if archive_lifecycle_complete(&logical_path, records)? {
        return Ok(());
    }
    for line in payload
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        SHARD_STORE
            .append_durable_line(&logical_path, line)
            .map_err(|error| {
                AxonError::runtime_failure(format!("写入归档分片文件失败: {error:?}"))
            })?;
    }
    Ok(())
}

/// 读取指定窗口分片并持有归档共享锁。
pub(crate) fn read_archive_records(
    session_dir: &Path,
    window_number: u64,
) -> AxonResult<Vec<ArchivedWindowRecord>> {
    read_archive_lines(
        &archive_logical_path(session_dir, window_number),
        window_number,
    )
}

/// 校验已提交 lifecycle 的单消息物理记录与原窗口消息完全一致。
pub(crate) fn archive_lifecycle_matches(
    session_dir: &Path,
    expected_records: &[ArchivedWindowRecord],
) -> AxonResult<bool> {
    if expected_records.is_empty() {
        return Ok(true);
    }
    validate_record_batch(expected_records)?;
    archive_lifecycle_complete(
        &archive_logical_path(session_dir, expected_records[0].window_number),
        expected_records,
    )
}

pub(crate) fn visit_archive_records_reverse(
    session_dir: &Path,
    window_number: u64,
    visitor: &mut dyn FnMut(&ArchivedWindowRecord) -> bool,
) -> AxonResult<ArchiveReverseScanReport> {
    let lines = SHARD_STORE
        .read_lines(archive_logical_path(session_dir, window_number))
        .map_err(|error| AxonError::runtime_failure(format!("打开归档分片失败: {error:?}")))?;
    let mut report = ArchiveReverseScanReport::default();
    for line in lines.iter().rev() {
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }
        match parse_physical_record(line, window_number) {
            Ok(record) => {
                if !visitor(&record) {
                    report.next_cursor = Some(format!(
                        "window={}&before_ordinal={}",
                        window_number, record.message_ordinal
                    ));
                    break;
                }
            }
            Err(_) => {
                report.corrupted_records = report.corrupted_records.saturating_add(1);
                report.partial = true;
                report
                    .next_cursor
                    .get_or_insert_with(|| format!("window={window_number}&repair-required"));
            }
        }
    }
    Ok(report)
}

fn archive_logical_path(session_dir: &Path, window_number: u64) -> std::path::PathBuf {
    session_dir
        .join(ARCHIVED_HISTORY_DIR_NAME)
        .join(archive_shard_file_name(window_number))
}

fn validate_record_batch(records: &[ArchivedWindowRecord]) -> AxonResult<()> {
    let Some(first) = records.first() else {
        return Ok(());
    };
    for record in records {
        validate_record(record)?;
        if record.lifecycle_id != first.lifecycle_id
            || record.window_number != first.window_number
            || record.window_id != first.window_id
            || record.previous_window_id != first.previous_window_id
        {
            return Err(AxonError::runtime_failure(
                "同一归档提交中的消息记录窗口身份不一致",
            ));
        }
    }
    Ok(())
}

fn validate_record(record: &ArchivedWindowRecord) -> AxonResult<()> {
    if record.schema_version != ARCHIVED_WINDOW_SCHEMA_VERSION {
        return Err(AxonError::runtime_failure(format!(
            "归档记录 schema 版本不支持: {}",
            record.schema_version
        )));
    }
    if record.checksum.is_empty() || record.checksum != archive_checksum(record) {
        return Err(AxonError::runtime_failure("归档记录校验和不匹配"));
    }
    Ok(())
}

fn encode_records(records: &[ArchivedWindowRecord]) -> AxonResult<Vec<u8>> {
    let mut payload = Vec::new();
    for record in records {
        serde_json::to_writer(&mut payload, record).map_err(|error| {
            AxonError::runtime_failure(format!("序列化归档记录失败: {error:?}"))
        })?;
        payload.push(b'\n');
    }
    Ok(payload)
}

fn archive_lifecycle_complete(
    path: &Path,
    expected_records: &[ArchivedWindowRecord],
) -> AxonResult<bool> {
    let lifecycle_id = expected_records
        .first()
        .map(|record| record.lifecycle_id.as_str())
        .ok_or_else(|| AxonError::runtime_failure("归档提交缺少消息记录"))?;
    let expected_window_number = expected_records
        .first()
        .map(|record| record.window_number)
        .unwrap_or_default();
    let lines = read_archive_lines(path, expected_window_number)?;
    let matched_records = lines
        .into_iter()
        .filter(|record| record.lifecycle_id == lifecycle_id)
        .collect::<Vec<_>>();
    if matched_records.is_empty() {
        return Ok(false);
    }
    if matched_records.len() == expected_records.len()
        && matched_records
            .iter()
            .zip(expected_records)
            .all(|(actual, expected)| same_archive_payload(actual, expected))
    {
        return Ok(true);
    }
    Err(AxonError::runtime_failure(format!(
        "归档生命周期 {lifecycle_id} 的消息集合不完整，拒绝推进窗口"
    )))
}

fn same_archive_payload(actual: &ArchivedWindowRecord, expected: &ArchivedWindowRecord) -> bool {
    actual.schema_version == expected.schema_version
        && actual.lifecycle_id == expected.lifecycle_id
        && actual.window_number == expected.window_number
        && actual.window_id == expected.window_id
        && actual.previous_window_id == expected.previous_window_id
        && actual.message == expected.message
        && actual.message_ordinal == expected.message_ordinal
}

fn parse_physical_record(
    bytes: &[u8],
    expected_window_number: u64,
) -> Result<ArchivedWindowRecord, String> {
    let record = serde_json::from_slice::<ArchivedWindowRecord>(bytes)
        .map_err(|error| format!("JSON 无效: {error}"))?;
    if record.schema_version != ARCHIVED_WINDOW_SCHEMA_VERSION {
        return Err(format!("schema 版本不支持: {}", record.schema_version));
    }
    if record.checksum.is_empty() || record.checksum != archive_checksum(&record) {
        return Err("校验和不匹配".to_owned());
    }
    if record.window_number != expected_window_number {
        return Err(format!(
            "窗口序号与分片不一致: expected {}, actual {}",
            expected_window_number, record.window_number
        ));
    }
    Ok(record)
}

fn invalid_record(line_number: usize, reason: &str) -> AxonError {
    AxonError::runtime_failure(format!("归档记录损坏（第 {line_number} 行）：{reason}"))
}

fn read_archive_lines(
    path: &Path,
    expected_window_number: u64,
) -> AxonResult<Vec<ArchivedWindowRecord>> {
    let lines = SHARD_STORE
        .read_lines(path)
        .map_err(|error| AxonError::runtime_failure(format!("读取归档分片失败: {error:?}")))?;
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.iter().all(|byte| byte.is_ascii_whitespace()))
        .map(|(index, line)| {
            parse_physical_record(line, expected_window_number)
                .map_err(|reason| invalid_record(index.saturating_add(1), reason.as_str()))
        })
        .collect()
}
