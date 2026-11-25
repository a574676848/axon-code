//! 整条日志的原子重写：正文分级之后把帧按新载荷重新打包。
//!
//! 追加路径只会在尾部写新帧，因此「把旧帧的正文换成引用」只能整体重写。重写必须
//! 满足三条：
//!
//! - **序号不变**：序号是控制面水位的事实，重写不允许改它，也不允许留下重复序号；
//! - **原子替换**：先写同目录临时文件、`fsync` 后再 `rename` 覆盖，崩溃时旧日志仍然完整；
//! - **索引同步**：帧偏移全变了，旁挂 `.idx` 必须按新布局重建，否则读路径会落到旧偏移。
//!
//! 记录条数不因重写变化，因此读侧的「跨帧序号连续」校验与重写前完全一致。

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::EventLog;
use crate::error::EventLogError;
use crate::format::{Header, encode_frame};
use crate::index::{self, FrameIndexEntry, record_interaction_tag};
use crate::lock::ExclusiveLock;
use crate::record::RuntimeEventRecord;

/// 重写时单个帧的明文预算。
///
/// 取 256 KiB：一次追加批次（流式刷新）通常远小于它，重写后帧数不会比原来更多，
/// 同时避免「一条记录一帧」把帧头与校验和的开销放大到正文量级。
const FRAME_PLAIN_BUDGET_BYTES: usize = 256 * 1024;

/// 重写结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewriteOutcome {
    /// 重写后的数据帧数（不含 header 帧）。
    pub frames: usize,
    /// 重写后的日志字节数。
    pub bytes: u64,
}

/// 用给定记录整体重写一条日志，并重建它的帧索引。
///
/// 记录必须按序号严格递增：重写保留原有序号，只改载荷与事件类型。空记录集只写
/// header 帧（调用方据此清空一条不再需要的日志）。
pub fn rewrite_log(
    log: &EventLog,
    header: &Header,
    records: &[RuntimeEventRecord],
) -> Result<RewriteOutcome, EventLogError> {
    validate_sequences(records)?;
    let path = log.path();
    // 与追加路径共用同一把锁：重写期间不允许别的写者往旧文件尾部追加，
    // 否则 rename 之后那些帧会凭空消失。
    let _lock = ExclusiveLock::acquire(&path.with_extension("lock"))?;
    write_locked(log, header, records)
}

/// 在读-改-写全程持有写锁地重写日志。
///
/// 调用方拿到的记录必须是**锁内读出的那一份**：先读后锁会漏掉其间新追加的帧，
/// 重写等于把它们删掉。`transform` 返回 `None` 表示无需改动，此时不写任何字节。
pub fn rewrite_log_in_place<F>(
    log: &EventLog,
    transform: F,
) -> Result<Option<RewriteOutcome>, EventLogError>
where
    F: FnOnce(
        &Header,
        Vec<RuntimeEventRecord>,
    ) -> Result<Option<Vec<RuntimeEventRecord>>, EventLogError>,
{
    let path = log.path();
    let _lock = ExclusiveLock::acquire(&path.with_extension("lock"))?;
    if !path.is_file() {
        return Ok(None);
    }
    let snapshot = crate::read::read_snapshot(&path)?;
    let Some(records) = transform(&snapshot.header, snapshot.records)? else {
        return Ok(None);
    };
    validate_sequences(&records)?;
    Ok(Some(write_locked(log, &snapshot.header, &records)?))
}

/// 用「锁外读出的快照」重写日志，并在锁内校验日志尾部没有被追加过。
///
/// 为什么要这个入口：把整条日志读进内存（解压 + 反序列化）可能持续数秒，主写锁不能
/// 被它按住，否则并发追加会在 5 秒锁超时后失败。这里把读放到锁外，锁内只做一次
/// 「末条序号是否仍是读快照时那一条」的核对：对不上就说明期间有人写过，本次放弃，
/// 由调用方下一轮重来。核对通过时，快照覆盖的正是文件当前内容，重写不会丢帧。
pub fn rewrite_log_if_unchanged(
    log: &EventLog,
    expected_tail_sequence: u64,
    header: &Header,
    records: &[RuntimeEventRecord],
) -> Result<Option<RewriteOutcome>, EventLogError> {
    validate_sequences(records)?;
    let path = log.path();
    let _lock = ExclusiveLock::acquire(&path.with_extension("lock"))?;
    if !path.is_file() {
        return Ok(None);
    }
    let tail = crate::read::read_tail_record(&path)?;
    if tail.map(|record| record.sequence) != Some(expected_tail_sequence) {
        return Ok(None);
    }
    Ok(Some(write_locked(log, header, records)?))
}

/// 已持有写锁时的落盘：编码、原子替换、重建索引。
fn write_locked(
    log: &EventLog,
    header: &Header,
    records: &[RuntimeEventRecord],
) -> Result<RewriteOutcome, EventLogError> {
    let path = log.path();
    let (bytes, entries) = encode_log(header, records)?;
    write_atomically(&path, &bytes)?;
    let log_len = u64::try_from(bytes.len())
        .map_err(|_| EventLogError::Corrupt("重写后的日志超出可寻址范围".to_owned()))?;
    index::store(&path, &entries, log_len)?;
    Ok(RewriteOutcome {
        frames: entries.len(),
        bytes: log_len,
    })
}

/// 序号必须严格递增：重复或倒退会让读者拿到非单调序列。
fn validate_sequences(records: &[RuntimeEventRecord]) -> Result<(), EventLogError> {
    let mut previous: Option<u64> = None;
    for record in records {
        if let Some(previous) = previous
            && record.sequence <= previous
        {
            return Err(EventLogError::Corrupt(format!(
                "重写记录的序号必须严格递增：{previous} 之后出现 {}",
                record.sequence
            )));
        }
        previous = Some(record.sequence);
    }
    Ok(())
}

/// 把 header 与记录编码成字节流，同时产出与新布局逐字节对齐的索引条目。
fn encode_log(
    header: &Header,
    records: &[RuntimeEventRecord],
) -> Result<(Vec<u8>, Vec<FrameIndexEntry>), EventLogError> {
    let mut bytes = encode_frame(&header.encode_line()?)?;
    let mut entries: Vec<FrameIndexEntry> = Vec::new();
    let mut index = 0usize;
    while index < records.len() {
        let first_index = index;
        let mut plain: Vec<u8> = Vec::with_capacity(FRAME_PLAIN_BUDGET_BYTES.min(4 * 1024));
        while index < records.len() && plain.len() < FRAME_PLAIN_BUDGET_BYTES {
            plain.extend_from_slice(&records[index].encode_line()?);
            plain.push(b'\n');
            index = index.saturating_add(1);
        }
        let frame = encode_frame(&plain)?;
        let offset = u64::try_from(bytes.len())
            .map_err(|_| EventLogError::Corrupt("重写偏移超出可寻址范围".to_owned()))?;
        let len = u32::try_from(frame.len())
            .map_err(|_| EventLogError::Corrupt("单个重写帧超出索引可表示范围".to_owned()))?;
        bytes.extend_from_slice(&frame);
        entries.push(FrameIndexEntry {
            first_sequence: records[first_index].sequence,
            offset,
            len,
            interaction_tag: frame_tag(&records[first_index..index]),
        });
    }
    Ok((bytes, entries))
}

/// 整帧同属一个交互时返回它的指纹，否则返回 0（跨交互）。
///
/// 与追加路径同口径：指纹只用于缩小解压范围，正确性由逐条判定兜底。
fn frame_tag(records: &[RuntimeEventRecord]) -> u32 {
    let mut tag: Option<u32> = None;
    for record in records {
        let current = record_interaction_tag(record);
        match tag {
            None => tag = Some(current),
            Some(previous) if previous == current => {}
            Some(_) => return 0,
        }
    }
    tag.unwrap_or(0)
}

/// 先写同目录临时文件再原子替换：崩溃时旧日志保持完整，不会读到半条新日志。
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), EventLogError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_data()
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        // 目标被并发读者占用（Windows 上表现为共享冲突）时保留旧日志，只清理临时文件。
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    // 日志已被整体替换：任何还指向旧文件偏移的常驻句柄都必须失效。
    crate::writer_registry::forget_session_log(path);
    Ok(())
}

/// 重写临时文件路径：与日志同目录，保证 `rename` 不跨卷。
fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".rewrite");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventLog, EventLogReader, RuntimeEventKind, read_after, read_header};

    fn record(sequence: u64, interaction: &str, payload: &str) -> RuntimeEventRecord {
        RuntimeEventRecord::new(
            sequence,
            1_000 + sequence,
            RuntimeEventKind::Other("wire_event".to_owned()),
            serde_json::json!({ "interactionId": interaction, "text": payload }),
        )
        .with_interaction(interaction)
    }

    fn seeded() -> (tempfile::TempDir, EventLog) {
        let directory = tempfile::tempdir().expect("临时目录");
        let log = EventLog::open(directory.path(), "session-rewrite");
        let header = Header::new("session-rewrite", 1, Some("/workspace".to_owned()));
        let records = vec![
            record(0, "interaction-a", "第一段"),
            record(1, "interaction-a", "第二段"),
            record(2, "interaction-b", "第三段"),
        ];
        rewrite_log(&log, &header, &records).expect("初次重写");
        (directory, log)
    }

    fn snapshot(log: &EventLog) -> EventLogReader {
        EventLogReader::open(&log.path()).expect("读回快照")
    }

    #[test]
    fn rewrite_replaces_payloads_and_keeps_sequences() {
        let (_directory, log) = seeded();
        let header = Header::new("session-rewrite", 1, Some("/workspace".to_owned()));
        let mut records = snapshot(&log).records().to_vec();
        records[1].payload = serde_json::json!({ "archived": true, "bytes": 5_120 });

        let outcome = rewrite_log(&log, &header, &records).expect("重写");

        let reader = snapshot(&log);
        assert_eq!(outcome.frames, 1, "三条记录应合并进同一帧");
        assert_eq!(
            reader
                .records()
                .iter()
                .map(|item| item.sequence)
                .collect::<Vec<_>>(),
            [0, 1, 2],
            "序号必须原样保留"
        );
        assert_eq!(reader.records()[1].payload["bytes"], 5_120);
        assert_eq!(reader.header().cwd.as_deref(), Some("/workspace"));
    }

    #[test]
    fn rewritten_index_keeps_keyed_reads_working() {
        let (_directory, log) = seeded();
        let header = Header::new("session-rewrite", 1, Some("/workspace".to_owned()));
        let mut records = snapshot(&log).records().to_vec();
        records[0].payload = serde_json::json!({ "archived": true });
        rewrite_log(&log, &header, &records).expect("重写");

        let tail = crate::read_interaction_tail(&log.path(), "interaction-a", 10).expect("按键读");
        assert_eq!(tail.len(), 2, "重建索引后按交互读取必须仍然命中");
        assert_eq!(tail[0].payload["archived"], true);
        assert_eq!(
            read_after(&log.path(), 0, 10).expect("按序号读").len(),
            2,
            "按序号读同样只跳过已读过的首条"
        );
    }

    #[test]
    fn duplicate_or_backwards_sequences_are_rejected() {
        let directory = tempfile::tempdir().expect("临时目录");
        let log = EventLog::open(directory.path(), "session-rewrite");
        let header = Header::new("session-rewrite", 1, None);
        for records in [
            vec![
                record(1, "interaction-a", "a"),
                record(1, "interaction-a", "b"),
            ],
            vec![
                record(2, "interaction-a", "a"),
                record(1, "interaction-a", "b"),
            ],
        ] {
            let error = rewrite_log(&log, &header, &records).expect_err("必须拒绝");
            assert!(matches!(error, EventLogError::Corrupt(_)), "{error:?}");
        }
    }

    #[test]
    fn empty_record_set_keeps_only_the_header_frame() {
        let (_directory, log) = seeded();
        let header = Header::new("session-rewrite", 1, Some("/workspace".to_owned()));
        let outcome = rewrite_log(&log, &header, &[]).expect("清空重写");
        assert_eq!(outcome.frames, 0);
        assert!(snapshot(&log).records().is_empty());
    }

    #[test]
    fn in_place_rewrite_skips_the_write_when_the_transform_declines() {
        let (_directory, log) = seeded();
        let before = std::fs::read(log.path()).expect("读日志");
        let outcome = crate::rewrite_log_in_place(&log, |_, _records| Ok(None)).expect("重写");
        assert!(outcome.is_none(), "无需改动时不应产生重写结果");
        assert_eq!(
            std::fs::read(log.path()).expect("读日志"),
            before,
            "无需改动时不得重写日志文件"
        );
    }

    #[test]
    fn in_place_rewrite_reports_absent_logs_instead_of_creating_them() {
        let directory = tempfile::tempdir().expect("临时目录");
        let log = EventLog::open(directory.path(), "session-absent");
        let outcome =
            crate::rewrite_log_in_place(&log, |_, _records| Ok(None)).expect("空日志不报错");
        assert!(outcome.is_none());
        assert!(!log.path().exists(), "不得凭空创建日志文件");
    }

    #[test]
    fn header_survives_a_rewrite_without_an_index() {
        let (_directory, log) = seeded();
        // 删掉索引：读路径要能只凭日志本身读出 header。
        std::fs::remove_file(index::index_path(&log.path())).expect("删索引");
        let header = read_header(&log.path())
            .expect("读 header")
            .expect("有 header");
        assert_eq!(header.session_id, "session-rewrite");
        assert_eq!(snapshot(&log).records().len(), 3);
    }
}
