//! 追加写入。

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::EventLog;
use crate::error::EventLogError;
use crate::format::{Header, encode_frame, scan_frames, tail_ends_on_frame_boundary};
use crate::index::{self, FrameIndexEntry, interaction_tag};
use crate::lock::ExclusiveLock;
use crate::record::RuntimeEventRecord;

/// 索引维护走同目录的 `append_index.rs`：打开既有日志时靠它免掉全量解压。
#[path = "append_index.rs"]
mod append_index;
use append_index::{ensure as ensure_index, restore_metadata};

/// 尾部自检窗口：正常日志只需解码最后一帧就能确认停在帧边界。
///
/// 只有当这一窗口里找不到可解码的尾帧时才回退全量扫描，因此稳态追加不会
/// 随日志增长变成 O(文件大小)。
const TAIL_SCAN_WINDOW_BYTES: u64 = 1 << 20;

/// 事件日志的追加写句柄。
///
/// 持有期间对同一日志保持跨进程独占。每次 `append_batch` 只做一次顺序写与一次
/// `fsync`；写失败时把文件截回写入前长度，避免留下半帧。
///
/// 三个细节：
/// - 日志文件延迟到首次写入才创建，避免"打开写句柄后崩溃"留下 0 字节文件；
/// - 打开时若发现上次崩溃留下的不完整尾帧，先截断并持久化，再接受新的追加；
/// - 每追加一帧同步维护旁挂帧索引，读路径据此二分定位起点帧（见 [`crate::index`]）。
#[derive(Debug)]
pub struct EventLogWriter {
    path: PathBuf,
    file: Option<File>,
    index_file: Option<File>,
    index_bytes: u64,
    /// 索引不可用（打不开或走不下去）：只影响读侧快慢，不影响事件落盘。
    index_disabled: bool,
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
            let committed = repair_torn_tail(&path, std::fs::metadata(&path)?.len())?;
            if committed == 0 {
                // 连第一个完整帧都没有：崩溃发生在写 header 的中途，整段重来。
                (Header::new(log.session_id(), now_ms(), None), 0, false, 0)
            } else {
                let (header, next_sequence) = restore_metadata(&path, committed)?;
                (header, next_sequence, true, committed)
            }
        } else {
            (Header::new(log.session_id(), now_ms(), None), 0, false, 0)
        };

        Ok(Self {
            path,
            file: None,
            index_file: None,
            index_bytes: 0,
            index_disabled: false,
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
        let index_before = self.index_bytes;
        if !self.header_written {
            let frame = encode_frame(&self.header.encode_line()?)?;
            if let Err(error) = self.write_frame(&frame, None, 0) {
                self.rollback(before, index_before);
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
        // 该帧首条事件的序号：索引按它二分，让读路径直接落在这一帧上。
        let first_sequence = events.first().map(|event| event.sequence);
        // 整批同属一个交互时记下指纹，读侧据此只解压该交互的帧；跨交互批次记 0。
        let batch_tag = batch_interaction_tag(events);
        if let Err(error) = self.write_frame(&frame, first_sequence, batch_tag) {
            self.rollback(before, index_before);
            return Err(error);
        }

        self.next_sequence = expected;
        Ok(expected.saturating_sub(1))
    }

    /// 顺序写一帧；`first_sequence` 非空时同步维护旁挂索引。
    ///
    /// 顺序固定为「帧 → 索引条目 → 索引覆盖长度」，因此崩溃只会让索引落后于日志。
    fn write_frame(
        &mut self,
        frame: &[u8],
        first_sequence: Option<u64>,
        interaction_tag: u32,
    ) -> Result<(), EventLogError> {
        let written = u64::try_from(frame.len())
            .map_err(|_| EventLogError::Corrupt("单帧超过可寻址长度".to_owned()))?;
        let offset = self.bytes_len;
        let covered = offset.saturating_add(written);
        self.ensure_files(first_sequence.is_some())?;
        if let Some(file) = self.file.as_mut() {
            file.write_all(frame)?;
            file.flush()?;
            file.sync_data()?;
        }
        if let Some(first_sequence) = first_sequence {
            let len = u32::try_from(written).map_err(|_| {
                EventLogError::Corrupt(format!("单帧长度超出索引可表示范围：{written} 字节"))
            })?;
            let mut index_bytes = self.index_bytes;
            if let Some(index_file) = self.index_file.as_mut() {
                // 索引是派生数据：只交给操作系统，不为它多付一次 `fsync`。崩溃后
                // 覆盖长度与日志对不上，下一次打开按日志重建即可。
                index::append(
                    index_file,
                    &mut index_bytes,
                    FrameIndexEntry {
                        first_sequence,
                        offset,
                        len,
                        interaction_tag,
                    },
                    covered,
                )?;
            }
            self.index_bytes = index_bytes;
        }
        self.bytes_len = covered;
        Ok(())
    }

    /// 首次写入时才落文件，未写入的日志在磁盘上不存在。
    fn ensure_files(&mut self, need_index: bool) -> Result<(), EventLogError> {
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
        if need_index && self.index_file.is_none() && !self.index_disabled {
            let path = index::index_path(&self.path);
            // 索引是派生数据：打不开就退化为「没有索引」，绝不因此挡住事件落盘。
            match OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => {
                    self.index_bytes = file.metadata().map(|meta| meta.len()).unwrap_or(0);
                    self.index_file = Some(file);
                }
                Err(_) => self.index_disabled = true,
            }
        }
        Ok(())
    }

    /// 把日志与索引截回写入前长度；截断失败时不掩盖原始错误。
    fn rollback(&mut self, before: u64, index_before: u64) {
        if let Some(file) = self.file.as_ref() {
            let _ = file.set_len(before);
            let _ = file.sync_data();
        }
        if let Some(index_file) = self.index_file.as_ref() {
            let _ = index_file.set_len(index_before);
            let _ = index_file.sync_data();
        }
        self.bytes_len = before;
        self.index_bytes = index_before;
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// 整批事件同属一个交互时返回它的指纹，否则返回 0（跨交互）。
///
/// 追加批次就是一次流式刷新，正常只含同一轮的帧；真的混了多个交互时退回 0，
/// 读侧会保守地把这一帧纳入候选，正确性不受影响。
fn batch_interaction_tag(events: &[RuntimeEventRecord]) -> u32 {
    let mut tag: Option<u32> = None;
    for event in events {
        let current = interaction_tag(event.interaction_id.as_deref());
        match tag {
            None => tag = Some(current),
            Some(previous) if previous == current => {}
            Some(_) => return 0,
        }
    }
    tag.unwrap_or(0)
}
impl EventLogWriter {
    /// 由调用方给定下一个序号打开写句柄，不按既有文件推断序号。
    ///
    /// 用于"控制面是序号权威"的场景：控制面已经保存 `last_event_sequence`，
    /// 这里直接按它续写。序号是否连续由 [`Self::append_batch`] 校验；文件缺失时
    /// 首次写入会自动补写 header 帧。
    ///
    /// 打开时同样先修复撕裂尾：不修就会把新帧写在撕裂字节之后，读侧扫到 torn
    /// 立即停止，之后写入的历史永久静默不可见。
    pub fn open_at(
        log: &EventLog,
        expected_next: u64,
        created_at_ms: u64,
    ) -> Result<Self, EventLogError> {
        let path = log.path();
        let lock = ExclusiveLock::acquire(&path.with_extension("lock"))?;
        let (bytes_len, header_written) = if path.exists() {
            let len = std::fs::metadata(&path)?.len();
            let repaired = repair_torn_tail(&path, len)?;
            ensure_index(&path, repaired)?;
            (repaired, repaired > 0)
        } else {
            (0, false)
        };
        Ok(Self {
            path,
            file: None,
            index_file: None,
            index_bytes: 0,
            index_disabled: false,
            _lock: lock,
            header: Header::new(log.session_id(), created_at_ms, None),
            next_sequence: expected_next,
            header_written,
            bytes_len,
        })
    }
}

/// 打开写句柄前修复撕裂尾，返回可以安全追加的字节长度。
///
/// 正常情况只读尾部窗口并解码最后一帧；窗口内找不到完整帧时回退全量帧扫描，
/// 用与读侧完全相同的判据定位截断点，保证写接口与 `read_snapshot` 口径一致。
fn repair_torn_tail(path: &Path, len: u64) -> Result<u64, EventLogError> {
    if len == 0 {
        return Ok(0);
    }
    let tail_start = len.saturating_sub(TAIL_SCAN_WINDOW_BYTES);
    let tail = read_range(path, tail_start, len)?;
    if tail_ends_on_frame_boundary(&tail) {
        return Ok(len);
    }
    let bytes = std::fs::read(path)?;
    let scan = scan_frames(&bytes)?;
    let Some(torn_at) = scan.torn_start else {
        return Ok(len);
    };
    let repaired = u64::try_from(torn_at).unwrap_or(0);
    let handle = OpenOptions::new().write(true).open(path)?;
    handle.set_len(repaired)?;
    handle.sync_data()?;
    Ok(repaired)
}

fn read_range(path: &Path, start: u64, end: u64) -> Result<Vec<u8>, EventLogError> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::with_capacity(usize::try_from(end - start).unwrap_or(0));
    file.take(end - start).read_to_end(&mut buffer)?;
    Ok(buffer)
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
