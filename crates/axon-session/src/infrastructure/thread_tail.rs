//! Thread 尾部读取：只取最近若干字节的事件，供「只要最新几轮」的读路径使用。
//!
//! 为什么需要它：thread 是 append-only JSONL，`read_thread` 会把整条历史读进内存。
//! 中途落盘、反思提示、轮序、最近 N 轮信号与用量增量投影都只需要尾部，按整条读会让
//! 这些「每轮一次」的路径退化成 O(历史)。尾部读取有明确字节预算，并在预算不足以覆盖
//! 调用方所需水位时如实报告「不完整」，由调用方决定是否回退全量读。

use std::path::Path;

use crate::domain::ThreadEventRecord;
use crate::infrastructure::jsonl_store::JsonlThreadEventStore;

/// 一次尾部读取的结果。
#[derive(Debug, Clone, Default)]
pub struct ThreadTailRead {
    /// 读到的记录，按事件顺序升序。
    pub records: Vec<ThreadEventRecord>,
    /// 是否覆盖了整条 thread（`true` 表示没有截断）。
    pub complete: bool,
}

impl JsonlThreadEventStore {
    /// 读取第一条分片（含 `ThreadStarted` 的会话头），用于补全 settings 与父子关系。
    pub fn read_thread_head(
        &self,
        root: &Path,
        thread_id: &str,
    ) -> Result<Vec<ThreadEventRecord>, String> {
        let paths = Self::thread_shard_paths(root, thread_id)?;
        match paths.first() {
            Some(path) => Self::read_file_public(path),
            None => Ok(Vec::new()),
        }
    }

    /// 从尾部向前读取，直到用满 `max_bytes` 或读完所有分片。
    ///
    /// 分片自身不超过 8 MiB，因此正常会话（末分片即最近活动）一次读就够了。尾行撕裂
    /// 时先受锁截断再重读一次：否则这一轮既读不到也写不进（写路径要先取序号）。
    pub fn read_thread_tail(
        &self,
        root: &Path,
        thread_id: &str,
        max_bytes: u64,
    ) -> Result<ThreadTailRead, String> {
        match self.read_thread_tail_unrepaired(root, thread_id, max_bytes) {
            Ok(read) => Ok(read),
            Err(error) => {
                Self::repair_torn_tail(root, thread_id)?;
                self.read_thread_tail_unrepaired(root, thread_id, max_bytes)
                    .map_err(|_| error)
            }
        }
    }

    fn read_thread_tail_unrepaired(
        &self,
        root: &Path,
        thread_id: &str,
        max_bytes: u64,
    ) -> Result<ThreadTailRead, String> {
        let paths = Self::thread_shard_paths(root, thread_id)?;
        if paths.is_empty() {
            return Ok(ThreadTailRead::default());
        }
        let mut budget = max_bytes;
        let mut chunks = Vec::<Vec<u8>>::new();
        let mut complete = true;
        for path in paths.iter().rev() {
            if budget == 0 {
                complete = false;
                break;
            }
            let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
            if size <= budget {
                let bytes = std::fs::read(path).map_err(|_| "无法读取 thread 事件".to_owned())?;
                budget = budget.saturating_sub(size);
                chunks.push(bytes);
                continue;
            }
            // 预算不足一个分片：只取末尾 budget 字节，并丢弃第一行（可能是半行）。
            let bytes = Self::read_file_tail_bytes(path, budget)?;
            budget = 0;
            complete = false;
            chunks.push(trim_partial_head(bytes));
        }
        chunks.reverse();
        let joined = chunks.concat();
        let records = Self::parse_lines_public(&joined)?;
        Ok(ThreadTailRead { records, complete })
    }

    fn read_file_tail_bytes(path: &Path, budget: u64) -> Result<Vec<u8>, String> {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let mut file = std::fs::File::open(path).map_err(|_| "无法读取 thread 事件".to_owned())?;
        let size = file
            .metadata()
            .map_err(|_| "无法读取 thread 事件".to_owned())?
            .len();
        let start = size.saturating_sub(budget);
        file.seek(SeekFrom::Start(start))
            .map_err(|_| "无法读取 thread 事件".to_owned())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| "无法读取 thread 事件".to_owned())?;
        Ok(bytes)
    }

    fn read_file_public(path: &Path) -> Result<Vec<ThreadEventRecord>, String> {
        let bytes = std::fs::read(path).map_err(|_| "无法读取 thread 事件".to_owned())?;
        Self::parse_lines_public(&bytes)
    }

    fn parse_lines_public(bytes: &[u8]) -> Result<Vec<ThreadEventRecord>, String> {
        let mut records = Vec::new();
        for raw_line in bytes.split(|byte| *byte == b'\n') {
            let raw_line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
            if raw_line.is_empty() {
                continue;
            }
            let record = serde_json::from_slice::<ThreadEventRecord>(raw_line)
                .map_err(|error| format!("thread 事件行无效：{error}"))?;
            records.push(record);
        }
        Ok(records)
    }
}

/// 丢掉被截断的第一行。
fn trim_partial_head(bytes: Vec<u8>) -> Vec<u8> {
    match bytes.iter().position(|byte| *byte == b'\n') {
        Some(index) => bytes[index.saturating_add(1)..].to_vec(),
        None => Vec::new(),
    }
}
impl JsonlThreadEventStore {
    /// 读取整条 thread；尾行撕裂时先受锁截断再重读一次。
    ///
    /// 撕裂尾行（崩溃或断电留下半行）会让解析整体失败；写路径分配序号前必须先读，若读
    /// 失败就永远走不到会截断尾行的 `repair_tail`，会话因此既不能恢复也不能继续。
    pub(crate) fn read_thread_full(
        root: &Path,
        thread_id: &str,
    ) -> Result<Vec<crate::domain::ThreadEventRecord>, String> {
        match Self::read_thread_full_unrepaired(root, thread_id) {
            Ok(records) => Ok(records),
            Err(error) => {
                Self::repair_torn_tail(root, thread_id)?;
                Self::read_thread_full_unrepaired(root, thread_id).map_err(|_| error)
            }
        }
    }

    fn read_thread_full_unrepaired(
        root: &Path,
        thread_id: &str,
    ) -> Result<Vec<crate::domain::ThreadEventRecord>, String> {
        let mut records = Vec::new();
        for path in Self::thread_shard_paths(root, thread_id)? {
            records.extend(Self::read_file_public(&path)?);
        }
        // 重写与删除旧分片之间崩溃时，同一序号可能出现两次：按序号稳定去重，
        // 排在前面（新分片）的那条胜出，避免重复轮次进入模型历史。
        records.sort_by_key(|record| record.sequence);
        records.dedup_by_key(|record| record.sequence);
        Ok(records)
    }
}

/// 反向尾扫是否已经能确定激活状态：碰到线程起点/检查点，或已确认没有未收口轮次。
pub(crate) fn activation_settled(reverse_records: &[ThreadEventRecord]) -> bool {
    use crate::domain::{ThreadEvent, TurnStatus};
    let mut pending = 0_i64;
    for record in reverse_records {
        match &record.event {
            ThreadEvent::ThreadStarted { .. } | ThreadEvent::ThreadCheckpoint { .. } => {
                return pending <= 0;
            }
            ThreadEvent::TurnStarted { .. } => pending -= 1,
            ThreadEvent::TurnCompleted { turn } => {
                if turn.status != TurnStatus::Active {
                    pending += 1;
                }
            }
            ThreadEvent::TurnFailed { .. } | ThreadEvent::TurnInterrupted { .. } => pending += 1,
            _ => {}
        }
        if pending <= 0 {
            return true;
        }
    }
    pending <= 0
}
