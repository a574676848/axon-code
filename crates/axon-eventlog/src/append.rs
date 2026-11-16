//! 追加写入。

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::EventLogError;
use crate::format::{encode_frame, Header};
use crate::lock::ExclusiveLock;
use crate::read::read_snapshot;
use crate::record::RuntimeEventRecord;
use crate::EventLog;

/// 事件日志的追加写句柄。
///
/// 持有期间对同一日志保持跨进程独占。每次 `append_batch` 只做一次顺序写与一次
/// `fsync`；写失败时把文件截回写入前长度，避免留下半帧。
///
/// 两个细节：
/// - 日志文件延迟到首次写入才创建，避免"打开写句柄后崩溃"留下 0 字节文件；
/// - 打开时若发现上次崩溃留下的不完整尾帧，先截断并持久化，再接受新的追加。
#[derive(Debug)]
pub struct EventLogWriter {
    path: PathBuf,
    file: Option<File>,
    _lock: ExclusiveLock,
    header: Header,
    next_sequence: u64,
    header_written: bool,
    bytes_len: u64,
}

impl EventLogWriter {
    pub(crate) fn open(log: &EventLog) -> Result<Self, EventLogError> {
        let path = log.path();
        let lock_path = path.with_extension("lock");
        let lock = ExclusiveLock::acquire(&lock_path)?;

        let (header, next_sequence, header_written, bytes_len) = if path.exists() {
            let snapshot = read_snapshot(&path)?;
            let mut committed = std::fs::metadata(&path)?.len();
            if let Some(torn_at) = snapshot.torn_truncate_to {
                let repaired = u64::try_from(torn_at).unwrap_or(committed);
                let handle = OpenOptions::new().write(true).open(&path)?;
                handle.set_len(repaired)?;
                handle.sync_data()?;
                committed = repaired;
            }
            let last = snapshot.records.last().map(|record| record.sequence);
            (
                snapshot.header,
                last.map_or(0, |sequence| sequence + 1),
                true,
                committed,
            )
        } else {
            (Header::new(log.session_id(), now_ms(), None), 0, false, 0)
        };

        Ok(Self {
            path,
            file: None,
            _lock: lock,
            header,
            next_sequence,
            header_written,
            bytes_len,
        })
    }

    /// 下一个应写入的序号。
    #[must_use]
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// 会话头部。
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// 追加一个批次，返回末条序号。
    ///
    /// 批次内序号必须从 `next_sequence` 起严格连续，否则整批拒绝且不写入任何字节。
    pub fn append_batch(&mut self, events: &[RuntimeEventRecord]) -> Result<u64, EventLogError> {
        if events.is_empty() {
            return Err(EventLogError::EmptyBatch);
        }
        let mut expected = self.next_sequence;
        for event in events {
            if event.sequence != expected {
                return Err(EventLogError::SequenceGap {
                    expected,
                    actual: event.sequence,
                });
            }
            expected = expected.saturating_add(1);
        }

        let before = self.bytes_len;
        if !self.header_written {
            let frame = encode_frame(&self.header.encode_line()?)?;
            if let Err(error) = self.write_frame(&frame) {
                self.rollback(before);
                return Err(error);
            }
            self.header_written = true;
        }

        let mut plain = Vec::new();
        for event in events {
            plain.extend_from_slice(&event.encode_line()?);
            plain.push(b'\n');
        }
        let frame = encode_frame(&plain)?;
        if let Err(error) = self.write_frame(&frame) {
            self.rollback(before);
            return Err(error);
        }

        self.next_sequence = expected;
        Ok(expected.saturating_sub(1))
    }

    fn write_frame(&mut self, frame: &[u8]) -> Result<(), EventLogError> {
        let file = self.ensure_file()?;
        file.write_all(frame)?;
        file.flush()?;
        file.sync_data()?;
        let written = u64::try_from(frame.len()).unwrap_or(u64::MAX);
        self.bytes_len = self.bytes_len.saturating_add(written);
        Ok(())
    }

    /// 首次写入时才落文件，未写入的日志在磁盘上不存在。
    fn ensure_file(&mut self) -> Result<&mut File, EventLogError> {
        if self.file.is_none() {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .append(true)
                .open(&self.path)?;
            self.file = Some(file);
        }
        self.file
            .as_mut()
            .ok_or_else(|| EventLogError::Io(std::io::Error::other("写句柄未就绪")))
    }

    /// 把文件截回写入前长度；截断失败时不掩盖原始错误。
    fn rollback(&mut self, before: u64) {
        if let Some(file) = self.file.as_ref() {
            let _ = file.set_len(before);
            let _ = file.sync_data();
        }
        self.bytes_len = before;
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
impl EventLogWriter {
    /// 由调用方给定下一个序号打开写句柄，不扫描既有文件。
    ///
    /// 用于"控制面是序号权威"的场景：控制面已经保存 `last_event_sequence`，
    /// 这里直接按它续写。序号是否连续由 [`Self::append_batch`] 校验；文件缺失时
    /// 首次写入会自动补写 header 帧。
    pub fn open_at(
        log: &EventLog,
        expected_next: u64,
        created_at_ms: u64,
    ) -> Result<Self, EventLogError> {
        let path = log.path();
        let lock = ExclusiveLock::acquire(&path.with_extension("lock"))?;
        let exists = path.exists();
        let bytes_len = if exists {
            std::fs::metadata(&path)?.len()
        } else {
            0
        };
        Ok(Self {
            path,
            file: None,
            _lock: lock,
            header: Header::new(log.session_id(), created_at_ms, None),
            next_sequence: expected_next,
            header_written: exists,
            bytes_len,
        })
    }
}

/// 一次性追加：持锁、追加、释放锁。
///
/// 适合"每次写入都从控制面取序号"的调用方；不保留跨批次的写句柄，
/// 因此多进程共享同一会话日志时不会互相阻塞。
pub fn append_once(
    log: &EventLog,
    next_sequence: u64,
    created_at_ms: u64,
    events: &[RuntimeEventRecord],
) -> Result<u64, EventLogError> {
    let mut writer = EventLogWriter::open_at(log, next_sequence, created_at_ms)?;
    writer.append_batch(events)
}
