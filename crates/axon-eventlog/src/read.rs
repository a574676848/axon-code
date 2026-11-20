//! 只读快照与查询。

use std::path::Path;

use crate::error::EventLogError;
use crate::format::{
    FrameSpan, HEADER_FRAME_INDEX, Header, decode_frame, scan_file_frames, scan_frames, step_frame,
};
use crate::index;
use crate::record::RuntimeEventRecord;

/// 一次完整读取的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct EventLogSnapshot {
    /// 会话头部。
    pub header: Header,
    /// 按序号升序的全部事件。
    pub records: Vec<RuntimeEventRecord>,
    /// 末尾不完整帧的起始字节；`Some` 表示日志尾部被截断。
    pub torn_truncate_to: Option<usize>,
}

/// 读取指定路径的完整快照。
///
/// 完整帧内出现半条记录、或中途解压失败，都按损坏处理并拒绝，不猜测内容。
pub fn read_snapshot(path: &Path) -> Result<EventLogSnapshot, EventLogError> {
    let bytes = std::fs::read(path)?;
    let scan = scan_frames(&bytes)?;
    let header_span = scan
        .frames
        .get(HEADER_FRAME_INDEX)
        .ok_or_else(|| EventLogError::Corrupt("日志缺少 header 帧".to_owned()))?;
    let header_plain = decode_frame(&bytes[header_span.start..header_span.end])?;
    let newlines = header_plain.iter().filter(|byte| **byte == b'\n').count();
    if newlines != 1 {
        return Err(EventLogError::Corrupt(format!(
            "header 帧必须恰好包含一行，实际换行数 {newlines}"
        )));
    }
    let header = Header::decode_line(&header_plain)?;

    let mut records = Vec::new();
    // 跨帧同样要求序号连续：只校验帧内连续会让「控制面水位倒退后写出的重复序号」
    // 静默通过，读者拿到的序号序列不再单调。
    let mut expected: Option<u64> = None;
    for span in scan.frames.iter().skip(HEADER_FRAME_INDEX + 1) {
        let plain = decode_frame(&bytes[span.start..span.end])?;
        for record in decode_records(&plain)? {
            if let Some(previous) = expected
                && record.sequence != previous
            {
                return Err(EventLogError::SequenceGap {
                    expected: previous,
                    actual: record.sequence,
                });
            }
            expected = Some(record.sequence.saturating_add(1));
            records.push(record);
        }
    }

    Ok(EventLogSnapshot {
        header,
        records,
        torn_truncate_to: scan.torn_start,
    })
}

/// 事件日志的只读读者。
#[derive(Debug, Clone, PartialEq)]
pub struct EventLogReader {
    snapshot: EventLogSnapshot,
}
/// 读取序号严格大于 `after` 的至多 `limit` 条事件。
///
/// 有索引时二分到起点帧后只解压需要的帧；没有索引（冷启、旧日志、索引落后）时从
/// 首帧单趟顺序解压并跳过起点之前的部分。两条路径都是单趟，不会为翻一页把整条
/// 日志重复解压。
pub fn read_after(
    path: &Path,
    after: u64,
    limit: usize,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(len) = log_len(path)? else {
        return Ok(Vec::new());
    };
    let mut file = std::fs::File::open(path)?;
    let start = index::load(path, len)
        .and_then(|index| index.locate(after))
        .map_or(0, |entry| entry.offset);
    collect_after(&mut file, start, len, after, limit)
}

/// 从 `start` 起顺序读取并收集序号大于 `after` 的事件。
fn collect_after(
    file: &mut std::fs::File,
    start: u64,
    total: u64,
    after: u64,
    limit: usize,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    let total = usize::try_from(total).unwrap_or(usize::MAX);
    let mut cursor = usize::try_from(start).unwrap_or(0);
    // 从文件起点开始时，第一帧是 header 帧：它只有一行 header，不是事件记录。
    let mut skip_header = cursor == 0;
    // 上限只为挡住调用方传入的极端值，不预分配 `limit` 条（可能是 usize::MAX）。
    let mut collected: Vec<RuntimeEventRecord> = Vec::with_capacity(limit.min(1024));
    while cursor < total {
        let Some(span) = step_frame(file, cursor, total)? else {
            break;
        };
        cursor = span.end;
        if skip_header {
            skip_header = false;
            continue;
        }
        for record in frame_records(file, &span)? {
            if record.sequence > after {
                collected.push(record);
                if collected.len() >= limit {
                    return Ok(collected);
                }
            }
        }
    }
    Ok(collected)
}

/// 日志文件长度；文件不存在时返回 `None`。
pub(crate) fn log_len(path: &Path) -> Result<Option<u64>, EventLogError> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.len())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// 从尾部倒推读取满足谓词的最后 `limit` 条事件，只解压必要的尾部帧。
pub fn read_tail_matching<F>(
    path: &Path,
    limit: usize,
    predicate: F,
) -> Result<Vec<RuntimeEventRecord>, EventLogError>
where
    F: Fn(&RuntimeEventRecord) -> bool,
{
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(len) = log_len(path)? else {
        return Ok(Vec::new());
    };
    let mut file = std::fs::File::open(path)?;
    let mut matched: Vec<RuntimeEventRecord> = Vec::new();
    // 有索引时按条目倒着解压，连帧结构扫描都省掉。
    if let Some(index) = index::load(path, len) {
        for entry in index.entries().iter().rev() {
            if matched.len() >= limit {
                break;
            }
            let (Ok(start), Ok(end)) =
                (usize::try_from(entry.offset), usize::try_from(entry.end()))
            else {
                break;
            };
            let span = FrameSpan { start, end };
            push_matches(
                &mut matched,
                frame_records(&mut file, &span)?,
                &predicate,
                limit,
            );
        }
        return Ok(matched);
    }
    let scan = scan_file_frames(&mut file)?;
    let frames: Vec<FrameSpan> = scan
        .frames
        .iter()
        .skip(HEADER_FRAME_INDEX + 1)
        .copied()
        .collect();
    for span in frames.iter().rev() {
        if matched.len() >= limit {
            break;
        }
        push_matches(
            &mut matched,
            frame_records(&mut file, span)?,
            &predicate,
            limit,
        );
    }
    Ok(matched)
}

/// 把一帧里命中谓词的事件按原序并入 `matched`，保持尾部窗口语义。
fn push_matches<F>(
    matched: &mut Vec<RuntimeEventRecord>,
    records: Vec<RuntimeEventRecord>,
    predicate: &F,
    limit: usize,
) where
    F: Fn(&RuntimeEventRecord) -> bool,
{
    let hits: Vec<RuntimeEventRecord> = records
        .into_iter()
        .filter(|record| predicate(record))
        .collect();
    for record in hits.into_iter().rev() {
        matched.insert(0, record);
    }
    if matched.len() > limit {
        matched.drain(..matched.len() - limit);
    }
}

/// 读取某条交互在 `after` 之后的至多 `limit` 条事件。
///
/// 索引条目带交互指纹，因此只解压「可能属于这条交互」的帧（指纹相同，或跨交互帧
/// 这种保守候选），不必为了一条交互把整段会话的帧全解一遍。指纹只是粗筛：命中后
/// 仍逐条比对 `interaction_id`，碰撞只会多解压一帧，不会返回别条交互的记录。
///
/// 没有索引（旧日志、索引落后）时退化为单趟顺序解压加逐条过滤。
pub fn read_interaction_after(
    path: &Path,
    interaction_id: &str,
    after: u64,
    limit: usize,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(len) = log_len(path)? else {
        return Ok(Vec::new());
    };
    let mut file = std::fs::File::open(path)?;
    let tag = index::interaction_tag(Some(interaction_id));
    let mut collected: Vec<RuntimeEventRecord> = Vec::with_capacity(limit.min(1024));
    if let Some(index) = index::load(path, len) {
        // 起点帧之后的条目才可能包含序号大于 `after` 的记录。
        let begin = index.locate(after).map_or(0, |entry| entry.offset);
        for entry in index.entries() {
            if collected.len() >= limit {
                break;
            }
            if entry.offset < begin || !entry.may_hold(tag) {
                continue;
            }
            let (Ok(start), Ok(end)) =
                (usize::try_from(entry.offset), usize::try_from(entry.end()))
            else {
                break;
            };
            for record in frame_records(&mut file, &FrameSpan { start, end })? {
                if record.sequence <= after || !record_belongs_to(&record, interaction_id) {
                    continue;
                }
                collected.push(record);
                if collected.len() >= limit {
                    break;
                }
            }
        }
        return Ok(collected);
    }
    let total = usize::try_from(len).unwrap_or(usize::MAX);
    let mut cursor = 0usize;
    let mut skip_header = true;
    while cursor < total {
        let Some(span) = step_frame(&mut file, cursor, total)? else {
            break;
        };
        cursor = span.end;
        if skip_header {
            skip_header = false;
            continue;
        }
        for record in frame_records(&mut file, &span)? {
            if record.sequence <= after || !record_belongs_to(&record, interaction_id) {
                continue;
            }
            collected.push(record);
            if collected.len() >= limit {
                return Ok(collected);
            }
        }
    }
    Ok(collected)
}

/// 从尾部倒推读取某条交互的最后 `limit` 条事件（按序号升序返回）。
pub fn read_interaction_tail(
    path: &Path,
    interaction_id: &str,
    limit: usize,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(len) = log_len(path)? else {
        return Ok(Vec::new());
    };
    let mut file = std::fs::File::open(path)?;
    let tag = index::interaction_tag(Some(interaction_id));
    let mut matched: Vec<RuntimeEventRecord> = Vec::new();
    if let Some(index) = index::load(path, len) {
        for entry in index.entries().iter().rev() {
            if matched.len() >= limit {
                break;
            }
            if !entry.may_hold(tag) {
                continue;
            }
            let (Ok(start), Ok(end)) =
                (usize::try_from(entry.offset), usize::try_from(entry.end()))
            else {
                break;
            };
            let records = frame_records(&mut file, &FrameSpan { start, end })?;
            push_belonging(&mut matched, records, interaction_id, limit);
        }
        return Ok(matched);
    }
    let scan = scan_file_frames(&mut file)?;
    for span in scan.frames.iter().skip(HEADER_FRAME_INDEX + 1).rev() {
        if matched.len() >= limit {
            break;
        }
        let records = frame_records(&mut file, span)?;
        push_belonging(&mut matched, records, interaction_id, limit);
    }
    Ok(matched)
}

/// 记录是否属于目标交互：优先归属列，老记录回落到载荷里的 `interactionId`。
///
/// 这条回退与网关侧的历史页判定同口径：升级前写入的事件没有归属列，只认列会让
/// 老交互整段打不开。
fn record_belongs_to(record: &RuntimeEventRecord, interaction_id: &str) -> bool {
    match record.interaction_id.as_deref() {
        Some(owner) => owner == interaction_id,
        None => index::payload_interaction_id(record) == Some(interaction_id),
    }
}

/// 把一帧里属于目标交互的记录按序并入尾部窗口，并保持不超过 `limit` 条。
fn push_belonging(
    matched: &mut Vec<RuntimeEventRecord>,
    records: Vec<RuntimeEventRecord>,
    interaction_id: &str,
    limit: usize,
) {
    let hits: Vec<RuntimeEventRecord> = records
        .into_iter()
        .filter(|record| record_belongs_to(record, interaction_id))
        .collect();
    for record in hits.into_iter().rev() {
        matched.insert(0, record);
    }
    if matched.len() > limit {
        matched.drain(..matched.len() - limit);
    }
}

/// 从尾部倒序扫描记录，由调用方决定何时停下。
///
/// 用途固定：历史分页要从最近若干轮里取读数与动作摘要，而「若干轮」换算成帧数随会话
/// 差异极大。按记录倒序回调，调用方在「已经覆盖到本页最早那一轮」时返回 `false` 即可
/// 收工：代价正比于这一页真实占用的帧数，与整条日志长度无关。`max_frames` 是硬上限，
/// 防住异常日志把一次翻页拖成整条会话解压。
///
/// 回调收到的是**最新的记录在前**；调用方需要升序时自行收集后反转。
pub fn read_tail_scan<F>(path: &Path, max_frames: usize, mut visit: F) -> Result<(), EventLogError>
where
    F: FnMut(&RuntimeEventRecord) -> bool,
{
    if max_frames == 0 {
        return Ok(());
    }
    let Some(len) = log_len(path)? else {
        return Ok(());
    };
    let mut file = std::fs::File::open(path)?;
    let mut frames = 0_usize;
    if let Some(index) = index::load(path, len) {
        for entry in index.entries().iter().rev() {
            if frames >= max_frames {
                break;
            }
            let (Ok(start), Ok(end)) =
                (usize::try_from(entry.offset), usize::try_from(entry.end()))
            else {
                break;
            };
            frames += 1;
            let records = frame_records(&mut file, &FrameSpan { start, end })?;
            if !visit_records_newest_first(&records, &mut visit) {
                return Ok(());
            }
        }
        return Ok(());
    }
    let scan = scan_file_frames(&mut file)?;
    let spans: Vec<FrameSpan> = scan
        .frames
        .iter()
        .skip(HEADER_FRAME_INDEX + 1)
        .rev()
        .take(max_frames)
        .cloned()
        .collect();
    for span in spans {
        let records = frame_records(&mut file, &span)?;
        if !visit_records_newest_first(&records, &mut visit) {
            return Ok(());
        }
    }
    Ok(())
}

/// 把一帧里的记录按「新到旧」喂给回调；返回回调是否要求继续。
fn visit_records_newest_first<F>(records: &[RuntimeEventRecord], visit: &mut F) -> bool
where
    F: FnMut(&RuntimeEventRecord) -> bool,
{
    for record in records.iter().rev() {
        if !visit(record) {
            return false;
        }
    }
    true
}

/// 从尾部倒读取最近若干帧里的全部记录（按序号升序返回）。
///
/// 用途固定：一页历史要给十几轮补读数与动作摘要，而这条日志可能很长。按帧从尾部倒推
/// 并封顶，代价只与「最近这几帧」成正比，与会话总体量无关；页里更早的轮次取不到时
/// 由调用方按「读数未知」降级，绝不为补一页把整条日志解一遍。
pub fn read_tail_records(
    path: &Path,
    max_frames: usize,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    if max_frames == 0 {
        return Ok(Vec::new());
    }
    let Some(len) = log_len(path)? else {
        return Ok(Vec::new());
    };
    let mut file = std::fs::File::open(path)?;
    let mut collected: Vec<RuntimeEventRecord> = Vec::new();
    if let Some(index) = index::load(path, len) {
        let entries = index.entries();
        let first = entries.len().saturating_sub(max_frames);
        for entry in entries[first..].iter().rev() {
            let (Ok(start), Ok(end)) =
                (usize::try_from(entry.offset), usize::try_from(entry.end()))
            else {
                break;
            };
            let records = frame_records(&mut file, &FrameSpan { start, end })?;
            for record in records.into_iter().rev() {
                collected.insert(0, record);
            }
        }
    } else {
        let scan = scan_file_frames(&mut file)?;
        for span in scan
            .frames
            .iter()
            .skip(HEADER_FRAME_INDEX + 1)
            .rev()
            .take(max_frames)
        {
            let records = frame_records(&mut file, span)?;
            for record in records.into_iter().rev() {
                collected.insert(0, record);
            }
        }
    }
    Ok(collected)
}

/// 读取落盘末条事件；日志不存在或没有完整帧时返回 `None`。
///
/// 会话恢复与写入补偿只要这一条事实：有索引时只解压末帧，不必为拿尾记录把整条
/// 日志解一遍。
pub fn read_tail_record(path: &Path) -> Result<Option<RuntimeEventRecord>, EventLogError> {
    let Some(len) = log_len(path)? else {
        return Ok(None);
    };
    let mut file = std::fs::File::open(path)?;
    if let Some(entry) = index::load(path, len).and_then(|index| index.last()) {
        let (Ok(start), Ok(end)) = (usize::try_from(entry.offset), usize::try_from(entry.end()))
        else {
            return Ok(None);
        };
        let span = FrameSpan { start, end };
        return Ok(frame_records(&mut file, &span)?.pop());
    }
    let scan = scan_file_frames(&mut file)?;
    let Some(span) = scan.frames.iter().skip(HEADER_FRAME_INDEX + 1).next_back() else {
        return Ok(None);
    };
    Ok(frame_records(&mut file, span)?.pop())
}

/// 读 header 时最多扫描的前缀字节数。
///
/// header 是一行小 JSON（含会话标识与工作目录），远小于这个上限；只在旁挂索引不可用
/// 时用得到，索引可用时按它的首帧偏移精确读取。
const HEADER_PREFIX_BYTES: u64 = 64 * 1024;

/// 读取日志头部；文件不存在或没有完整 header 帧时返回 `None`。
///
/// 维护任务按会话给日志分级时要先知道它的真实会话标识：事件目录名是转义后的片段，
/// 不可逆，而 header 里的 `sessionId` 就是权威事实。
pub fn read_header(path: &Path) -> Result<Option<Header>, EventLogError> {
    let Some(len) = log_len(path)? else {
        return Ok(None);
    };
    if len == 0 {
        return Ok(None);
    }
    let prefix_len = index::load(path, len)
        .map(|index| index.header_end(len))
        .unwrap_or(HEADER_PREFIX_BYTES)
        .min(len);
    let bytes = index::read_range(path, 0, prefix_len)?;
    let scan = scan_frames(&bytes)?;
    let Some(span) = scan.frames.first() else {
        return Ok(None);
    };
    Ok(decode_header_frame(&bytes[span.start..span.end]))
}

/// 解出 header 帧：必须恰好一行，且版本受支持。
pub(crate) fn decode_header_frame(bytes: &[u8]) -> Option<Header> {
    let plain = decode_frame(bytes).ok()?;
    let newlines = plain.iter().filter(|byte| **byte == b'\n').count();
    if newlines != 1 {
        return None;
    }
    Header::decode_line(&plain).ok()
}

/// 读出并解压单帧：只把这一帧的字节搬进内存。
pub(crate) fn frame_records(
    file: &mut std::fs::File,
    span: &FrameSpan,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let len = span.end.saturating_sub(span.start);
    file.seek(SeekFrom::Start(
        u64::try_from(span.start).unwrap_or(u64::MAX),
    ))?;
    let mut buffer = vec![0u8; len];
    file.read_exact(&mut buffer)?;
    decode_records(&decode_frame(&buffer)?)
}

/// 把一帧明文拆成事件记录，并校验帧内序号连续。
fn decode_records(plain: &[u8]) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    // 起始序号由控制面决定，这里只校验相邻连续，不假定从 0 开始。
    let mut expected: Option<u64> = None;
    let mut records = Vec::new();
    for line in plain.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let record = RuntimeEventRecord::decode_line(line)?;
        if let Some(previous) = expected
            && record.sequence != previous
        {
            return Err(EventLogError::SequenceGap {
                expected: previous,
                actual: record.sequence,
            });
        }
        expected = Some(record.sequence.saturating_add(1));
        records.push(record);
    }
    Ok(records)
}

impl EventLogReader {
    /// 打开并解析一个日志文件。
    pub fn open(path: &Path) -> Result<Self, EventLogError> {
        Ok(Self {
            snapshot: read_snapshot(path)?,
        })
    }

    /// 用已有快照构造读者。
    #[must_use]
    pub fn from_snapshot(snapshot: EventLogSnapshot) -> Self {
        Self { snapshot }
    }

    /// 会话头部。
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.snapshot.header
    }

    /// 全部事件。
    #[must_use]
    pub fn records(&self) -> &[RuntimeEventRecord] {
        &self.snapshot.records
    }

    /// 末条事件的序号。
    #[must_use]
    pub fn last_sequence(&self) -> Option<u64> {
        self.snapshot.records.last().map(|record| record.sequence)
    }

    /// 读取序号严格大于 `after` 的至多 `limit` 条事件。
    #[must_use]
    pub fn read_from(&self, after: u64, limit: usize) -> Vec<RuntimeEventRecord> {
        self.snapshot
            .records
            .iter()
            .filter(|record| record.sequence > after)
            .take(limit)
            .cloned()
            .collect()
    }

    /// 读取尾部至多 `limit` 条事件，按序号升序返回。
    #[must_use]
    pub fn read_tail(&self, limit: usize) -> Vec<RuntimeEventRecord> {
        let total = self.snapshot.records.len();
        let start = total.saturating_sub(limit);
        self.snapshot.records[start..].to_vec()
    }

    /// 按谓词过滤读取，用于按 interaction 定向取帧。
    #[must_use]
    pub fn read_matching<F>(&self, limit: usize, predicate: F) -> Vec<RuntimeEventRecord>
    where
        F: Fn(&RuntimeEventRecord) -> bool,
    {
        self.snapshot
            .records
            .iter()
            .filter(|record| predicate(record))
            .take(limit)
            .cloned()
            .collect()
    }
}
