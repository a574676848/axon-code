//! 追加写入。

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::EventLog;
use crate::error::EventLogError;
use crate::format::{Header, encode_frame, scan_frames, tail_ends_on_frame_boundary};
use crate::index::{self, FrameIndex, FrameIndexEntry, record_interaction_tag};
use crate::lock::ExclusiveLock;
use crate::read::read_tail_record;
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

/// 单次追加的慢阈值：超过即按 WARNING 留一条分段记录，供事后归因。
const SLOW_APPEND_MS: u128 = 100;

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
    /// 排他锁：常驻复用的句柄在批次之间释放，写时再取；一次性句柄整段持有。
    lock: Option<ExclusiveLock>,
    header: Header,
    next_sequence: u64,
    header_written: bool,
    bytes_len: u64,
    /// 常驻内存的帧索引：打开时加载一次，此后每写一帧只追加条目。
    ///
    /// 这是常驻写者省掉「每批两次整份索引读」的关键：条目由写路径自己维护，
    /// 读路径仍以磁盘索引为准，两者的一致性由「日志只追加」这一前提加长度核对保证。
    index_state: Option<FrameIndex>,
    /// 句柄代际：日志被整体重写后，注册表会把代际推进一格。
    ///
    /// 仅靠「文件长度 + 尾号」判不出「文件已被换成另一份、恰好等长」这种情况，
    /// 代际是那条路径上唯一可靠的失效信号。一次性句柄恒为注册表给的同一代际。
    generation: u64,
    /// 上次写入后的文件修改时刻，用来识别「等长替换」。
    ///
    /// 重写日志会改变修改时刻，即使新旧字节数恰好相同；只看长度会让常驻句柄把帧
    /// 写进已经被换掉的那份文件，调用方还会拿到成功。`None` 表示尚未取样。
    modified: Option<std::time::SystemTime>,
    /// 最近一批追加的分段耗时：句柄与索引打开、写帧、fsync。
    ///
    /// 只在慢追加时随告警一起打出来，平时不做任何额外 IO。等锁与持锁总量由
    /// [`Self::append_batch_reusing`] 单独记录，两者合起来才能把一次慢追加归因到段。
    segments: AppendSegments,
}

/// 一批追加内部的分段耗时，单位微秒。
///
/// 六段口径与写路径一一对应：对齐（撕裂尾修复与索引重建）、编码（JSON 行与 zstd 压缩）、
/// 打开（日志与索引句柄）、写帧（`write_all` 与 `flush`）、fsync、索引写。等锁与持锁总量
/// 由 [`EventLogWriter::append_batch_reusing`] 单独记录，两者合起来才能把一次慢追加归因。
#[derive(Debug, Default, Clone, Copy)]
struct AppendSegments {
    /// 与磁盘重新对齐：撕裂尾修复、索引重建、尾号重算。
    align_us: u128,
    /// JSON 行与 zstd 帧编码。
    encode_us: u128,
    /// 首次写入时打开日志与索引句柄。
    open_us: u128,
    /// 写帧并 flush。
    write_us: u128,
    /// 逐帧 `sync_data`。
    fsync_us: u128,
    /// 旁挂帧索引的条目写入。
    index_us: u128,
}

impl AppendSegments {
    fn summary(&self) -> String {
        format!(
            "对齐={:.1}ms 编码={:.1}ms 打开={:.1}ms 写帧={:.1}ms fsync={:.1}ms 索引={:.1}ms",
            self.align_us as f64 / 1000.0,
            self.encode_us as f64 / 1000.0,
            self.open_us as f64 / 1000.0,
            self.write_us as f64 / 1000.0,
            self.fsync_us as f64 / 1000.0,
            self.index_us as f64 / 1000.0
        )
    }
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
            lock: Some(lock),
            header,
            next_sequence,
            header_written,
            bytes_len,
            index_state: None,
            generation: 0,
            modified: None,
            segments: AppendSegments::default(),
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
            let encode_started = std::time::Instant::now();
            let frame = encode_frame(&self.header.encode_line()?)?;
            self.segments.encode_us += encode_started.elapsed().as_micros();
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
        let encode_started = std::time::Instant::now();
        let frame = encode_frame(&plain)?;
        self.segments.encode_us += encode_started.elapsed().as_micros();
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
        let open_started = std::time::Instant::now();
        self.ensure_files(first_sequence.is_some())?;
        self.segments.open_us += open_started.elapsed().as_micros();
        if let Some(file) = self.file.as_mut() {
            let write_started = std::time::Instant::now();
            file.write_all(frame)?;
            file.flush()?;
            self.segments.write_us += write_started.elapsed().as_micros();
            let sync_started = std::time::Instant::now();
            file.sync_data()?;
            self.segments.fsync_us += sync_started.elapsed().as_micros();
        }
        if let Some(first_sequence) = first_sequence {
            let len = u32::try_from(written).map_err(|_| {
                EventLogError::Corrupt(format!("单帧长度超出索引可表示范围：{written} 字节"))
            })?;
            let mut index_bytes = self.index_bytes;
            if let Some(index_file) = self.index_file.as_mut() {
                // 索引是派生数据：只交给操作系统，不为它多付一次 `fsync`。崩溃后
                // 覆盖长度与日志对不上，下一次打开按日志重建即可。
                let index_started = std::time::Instant::now();
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
                self.segments.index_us += index_started.elapsed().as_micros();
            }
            // 内存索引与磁盘索引同一顺序维护：常驻句柄下一次追加要据此判断尾号与偏移，
            // 不再回头读整份索引文件。
            self.index_state
                .get_or_insert_with(FrameIndex::default)
                .push(FrameIndexEntry {
                    first_sequence,
                    offset,
                    len,
                    interaction_tag,
                });
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
        /*
         * header 帧也被截掉了：下次写入必须重新补写。回滚到 0 之外的偏移说明 header
         * 仍在，保持原状态即可。
         */
        if before == 0 {
            self.header_written = false;
        }
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
        let current = record_interaction_tag(event);
        match tag {
            None => tag = Some(current),
            Some(previous) if previous == current => {}
            Some(_) => return 0,
        }
    }
    tag.unwrap_or(0)
}
impl EventLogWriter {
    /// 由调用方给定下一个序号打开写句柄，并与文件尾对齐后才允许写入。
    ///
    /// 用于"控制面是序号权威"的场景：控制面已经保存 `last_event_sequence`，
    /// 这里按它续写。文件缺失时首次写入会自动补写 header 帧。
    ///
    /// 打开时做两件事：
    /// - 先修复撕裂尾：不修就会把新帧写在撕裂字节之后，读侧扫到 torn 立即停止，
    ///   之后写入的历史永久静默不可见；
    /// - 再在**持锁之后**核对文件尾：调用方声明的序号必须正好接在已落盘的最后一条
    ///   之后，否则返回 [`EventLogError::SequenceTailMismatch`] 且不写任何字节。
    ///
    /// 第二步不能省：锁只保证字节串行，不保证「序号与落点」对应。同一会话的两个写者
    /// 各自从控制面领号后，谁先拿到文件锁谁先落盘，后落盘的持有更大的号却落在更大的
    /// 偏移上，读者看到的序号就会逆序（现场 6231 条里有 28 处）。
    ///
    /// 空日志（没有任何记录，含「只有 header 帧」）没有尾号可比对，只能校验「声明的
    /// 是日志开头」：0 是事件日志层的原点（[`EventLogWriter::open`] 从 0 起写），1 是
    /// 控制面首次分配到的号（`last_event_sequence=0` 加一），两种起点都合规。
    pub fn open_at(
        log: &EventLog,
        expected_next: u64,
        created_at_ms: u64,
    ) -> Result<Self, EventLogError> {
        let path = log.path();
        let lock = ExclusiveLock::acquire(&path.with_extension("lock"))?;
        let mut index_state = None;
        let (bytes_len, header_written) = if path.exists() {
            let len = std::fs::metadata(&path)?.len();
            let repaired = repair_torn_tail(&path, len)?;
            ensure_index(&path, repaired)?;
            /*
             * 索引只在这里完整加载一次：常驻句柄此后按「写一帧追加一条」维护内存副本，
             * 不再回头读整份索引。旧实现里同一份索引在每次 append 里要被读两遍，
             * 长会话下这两次读就是写路径上最大的固定开销。
             */
            index_state = index::load(&path, repaired);
            (repaired, repaired > 0)
        } else {
            (0, false)
        };
        let durable_tail = durable_tail_sequence(&path)?;
        let accepted = match durable_tail {
            Some(tail) => expected_next == tail.saturating_add(1),
            None => expected_next <= 1,
        };
        if !accepted {
            return Err(EventLogError::SequenceTailMismatch {
                expected: expected_next,
                actual: durable_tail.unwrap_or(0),
            });
        }
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        Ok(Self {
            path,
            file: None,
            index_file: None,
            index_bytes: 0,
            index_disabled: false,
            lock: Some(lock),
            header: Header::new(log.session_id(), created_at_ms, None),
            next_sequence: expected_next,
            header_written,
            bytes_len,
            index_state,
            generation: 0,
            modified,
            segments: AppendSegments::default(),
        })
    }
}

impl EventLogWriter {
    /// 以「可复用」方式打开：校验完成后立即释放排他锁。
    ///
    /// 常驻写者用它在建句柄时付一次完整校验（撕裂尾修复、索引加载、尾号核对），
    /// 之后每次追加由 [`Self::append_batch_reusing`] 短暂取锁。锁仍然只覆盖一次追加，
    /// 因此多进程之间「先到先写、后到按尾号校验」的语义与一次性句柄完全一致。
    pub(crate) fn open_reusable(
        log: &EventLog,
        expected_next: u64,
        created_at_ms: u64,
        generation: u64,
    ) -> Result<Self, EventLogError> {
        let mut writer = Self::open_at(log, expected_next, created_at_ms)?;
        writer.generation = generation;
        writer.release_lock();
        Ok(writer)
    }

    /// 释放排他锁，留下可复用的句柄与常驻索引。
    fn release_lock(&mut self) {
        self.lock = None;
    }

    /// 复用句柄追加一批：短暂取锁、核对磁盘、写帧、释放锁。
    ///
    /// 核对只用一次 `metadata` 与内存里的尾号：
    /// - 文件长度与内存记录一致且序号相接时，说明没有别的进程插写，直接落盘；
    /// - 任一不符时重新与磁盘对齐（修复撕裂尾、重建索引、重算尾号），再按对齐结果
    ///   决定接受还是返回 [`EventLogError::SequenceTailMismatch`]。
    pub fn append_batch_reusing(
        &mut self,
        expected_next: u64,
        generation: u64,
        events: &[RuntimeEventRecord],
    ) -> Result<u64, EventLogError> {
        // 代际不符说明日志已被整体重写：旧句柄指向的文件已经不是当前日志。
        if self.generation != generation {
            return Err(EventLogError::StaleHandle);
        }
        if self.lock.is_none() {
            let started = std::time::Instant::now();
            self.lock = Some(ExclusiveLock::acquire(&self.path.with_extension("lock"))?);
            let lock_wait_ms = started.elapsed().as_millis();
            if lock_wait_ms >= SLOW_APPEND_MS {
                axon_logger::warning(&format!(
                    "事件日志抢锁偏慢：log={} 等锁={}ms",
                    self.path.display(),
                    lock_wait_ms
                ));
            }
        }
        self.segments = AppendSegments::default();
        let started = std::time::Instant::now();
        let outcome = self.append_locked(expected_next, events);
        self.release_lock();
        let elapsed_ms = started.elapsed().as_millis();
        if elapsed_ms >= SLOW_APPEND_MS {
            /*
             * 分段口径只说清楚「慢在多长时间、写了几帧」：等锁已在上一条里单独留痕，
             * 这里再给出持锁内的总耗时，两者相减即写帧与对齐的净成本。
             */
            axon_logger::warning(&format!(
                "事件日志追加偏慢：log={} 持锁耗时={}ms 帧数={} 分段：{}",
                self.path.display(),
                elapsed_ms,
                events.len(),
                self.segments.summary()
            ));
        }
        outcome
    }

    /// 持锁状态下的追加主体；错误路径由调用方统一释放锁。
    fn append_locked(
        &mut self,
        expected_next: u64,
        events: &[RuntimeEventRecord],
    ) -> Result<u64, EventLogError> {
        let metadata = std::fs::metadata(&self.path).ok();
        let actual_len = metadata.as_ref().map(|meta| meta.len()).unwrap_or(0);
        let actual_modified = metadata.as_ref().and_then(|meta| meta.modified().ok());
        /*
         * 三项判据缺一不可：长度发现追加，修改时刻发现「等长替换」（重写日志会改变
         * 修改时刻），尾号发现控制面与文件的不同步。只看长度会让等长重写后的常驻
         * 句柄继续写旧文件，且调用方拿到成功。
         */
        let length_changed = actual_len != self.bytes_len;
        let modification_changed = match (self.modified, actual_modified) {
            (Some(known), Some(actual)) => known != actual,
            _ => false,
        };
        if length_changed || modification_changed || self.next_sequence != expected_next {
            let align_started = std::time::Instant::now();
            self.resync(expected_next)?;
            self.segments.align_us += align_started.elapsed().as_micros();
        }
        self.next_sequence = expected_next;
        let outcome = self.append_batch(events);
        if outcome.is_ok() {
            self.modified = self
                .file
                .as_ref()
                .and_then(|file| file.metadata().ok())
                .and_then(|meta| meta.modified().ok());
        }
        outcome
    }

    /// 重新与磁盘对齐：修复撕裂尾、重建索引、重算尾号。
    fn resync(&mut self, expected_next: u64) -> Result<(), EventLogError> {
        // 重新对齐要重新打开文件与索引句柄：旧句柄的写入位置与索引字节数都已失效。
        self.file = None;
        self.index_file = None;
        self.index_disabled = false;
        let len = std::fs::metadata(&self.path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        let repaired = repair_torn_tail(&self.path, len)?;
        ensure_index(&self.path, repaired)?;
        self.index_state = index::load(&self.path, repaired);
        /*
         * 索引只覆盖「已落盘的数据帧」：覆盖长度必须正好等于修复后的文件长度。
         * `index::load` 已经这样校验，这里再核一次是防御性自检，避免将来放宽校验后
         * 常驻索引带着过期偏移继续被写路径使用。
         */
        if self
            .index_state
            .as_ref()
            .is_some_and(|state| state.covered_len() != repaired)
        {
            self.index_state = None;
        }
        self.index_bytes = std::fs::metadata(index::index_path(&self.path))
            .map(|meta| meta.len())
            .unwrap_or(0);
        if repaired == 0 {
            // 空日志：0 与 1 都是合规起点，与 `open_at` 同一口径。
            if expected_next > 1 {
                return Err(EventLogError::SequenceTailMismatch {
                    expected: expected_next,
                    actual: 0,
                });
            }
            self.header_written = false;
            self.bytes_len = 0;
            self.next_sequence = expected_next;
            self.modified = std::fs::metadata(&self.path)
                .ok()
                .and_then(|metadata| metadata.modified().ok());
            return Ok(());
        }
        let (header, durable_next) = restore_metadata(&self.path, repaired)?;
        self.header = header;
        self.header_written = true;
        self.bytes_len = repaired;
        /*
         * 无论接受还是拒绝，句柄都要反映磁盘上的真实尾号：拒绝路径上如果把它留在旧值，
         * 下一次追加会因为「尾号不等」再走一遍对齐，并发冲突时就会反复付全量重建的代价，
         * 表现为活锁。这里先落真实尾号，再由调用方按返回的错误重算并重试。
         */
        self.next_sequence = durable_next;
        self.modified = std::fs::metadata(&self.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if durable_next != expected_next {
            return Err(EventLogError::SequenceTailMismatch {
                expected: expected_next,
                actual: durable_next.saturating_sub(1),
            });
        }
        Ok(())
    }
}

/// 文件尾序号：已落盘的最后一条记录序号；`None` 表示日志还没有任何记录。
///
/// header 帧不是记录，只有 header 的日志按空日志处理。解码复用读侧的
/// [`read_tail_record`]，不另写一套格式解析：有索引时只解压末帧，没有索引时扫一遍
/// 帧结构。
fn durable_tail_sequence(path: &Path) -> Result<Option<u64>, EventLogError> {
    Ok(read_tail_record(path)?.map(|record| record.sequence))
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
///
/// `next_sequence` 必须正好接在文件尾之后（空日志允许事件日志层原点 0 或控制面首次
/// 分配的 1）。与文件尾不一致时返回 [`EventLogError::SequenceTailMismatch`] 且不写任何
/// 字节：调用方据此回滚控制面水位、按文件尾重算后重试，而不是把序号逆序的帧静默落盘。
pub fn append_once(
    log: &EventLog,
    next_sequence: u64,
    created_at_ms: u64,
    events: &[RuntimeEventRecord],
) -> Result<u64, EventLogError> {
    crate::writer_registry::append_to_session(log, next_sequence, created_at_ms, events)
}
