//! 持久 Shell 会话的有界输出缓冲与完整归档。

use std::{
    collections::VecDeque,
    fs::File,
    io::Write,
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axon_crypto::Sha256Hex;

use super::{
    APPROX_CHARS_PER_TOKEN, ShellCommandBoundedStream, ShellCommandOutputChunk,
    ShellCommandOutputStream,
};

const OUTPUT_HEAD_BYTES: usize = 16 * 1024;
const OUTPUT_TAIL_BYTES: usize = 16 * 1024;
const OUTPUT_OMITTED_MARKER: &[u8] = b"\n... output omitted; full output is in the artifact ...\n";
/// 归档分帧的最大固定开销：`\n----- ` + ` seq=` + u64 最长十进制位数 + ` -----\n`。
///
/// 按最大值预留使容量判定偏保守，归档文件大小因此不会越过上限。
const ARTIFACT_FRAME_MAX_OVERHEAD_BYTES: usize = 39;

pub(crate) struct SharedProcessOutput {
    stdout: Mutex<StreamState>,
    stderr: Mutex<StreamState>,
    events: Mutex<VecDeque<ShellCommandOutputChunk>>,
    event_bytes: Mutex<usize>,
    artifact: Mutex<File>,
    artifact_failed: AtomicBool,
    artifact_bytes: AtomicUsize,
    artifact_truncated: AtomicBool,
    max_artifact_bytes: usize,
    sequence: AtomicU64,
    change: ProcessOutputChange,
}

impl SharedProcessOutput {
    /// `max_artifact_bytes` 为归档落盘上限；达到后永久停止写入并标记截断。
    ///
    /// 归档创建时已写入 command/cwd/tty 头部，其长度计入总量，
    /// 因此上限约束的是归档文件本身的大小。
    pub(crate) fn new(artifact: File, max_artifact_bytes: usize) -> Self {
        let artifact_bytes = artifact
            .metadata()
            .map_or(0, |meta| usize::try_from(meta.len()).unwrap_or(usize::MAX));
        Self {
            stdout: Mutex::new(StreamState::default()),
            stderr: Mutex::new(StreamState::default()),
            events: Mutex::new(VecDeque::new()),
            event_bytes: Mutex::new(0),
            artifact: Mutex::new(artifact),
            artifact_failed: AtomicBool::new(false),
            artifact_bytes: AtomicUsize::new(artifact_bytes),
            artifact_truncated: AtomicBool::new(false),
            max_artifact_bytes,
            sequence: AtomicU64::new(0),
            change: ProcessOutputChange::default(),
        }
    }

    pub(crate) fn append(&self, stream: ShellCommandOutputStream, bytes: &[u8]) {
        self.append_with_display(stream, bytes, bytes);
    }

    /// Archive raw terminal output while exposing only its readable projection to the Agent.
    pub(crate) fn append_terminal(
        &self,
        stream: ShellCommandOutputStream,
        raw_bytes: &[u8],
        display_bytes: &[u8],
    ) {
        self.append_with_display(stream, raw_bytes, display_bytes);
    }

    fn append_with_display(
        &self,
        stream: ShellCommandOutputStream,
        raw_bytes: &[u8],
        display_bytes: &[u8],
    ) {
        if raw_bytes.is_empty() {
            return;
        }
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let stream_name = match stream {
            ShellCommandOutputStream::Stdout => "stdout",
            ShellCommandOutputStream::Stderr => "stderr",
        };
        // 达到上限后永久停止落盘：一次性截断而非环形覆盖，避免归档内容与 seq 标记错位。
        // 已截断的归档不再恢复写入，否则后续小负载会补进归档，造成帧号不连续。
        //
        // 上限判定与计数累加**必须在同一临界区**内完成：二者保护的是同一个归档文件，
        // 而 stdout / stderr 各有一个 reader 线程在并发调用本函数。锁外读计数快照、
        // 锁内用陈旧值回写会丢失并发增量（lost update），使计数系统性偏低、上限被
        // 推迟触发：归档最坏可涨到约 2× 上限。
        if !self.artifact_truncated.load(Ordering::Acquire) {
            let pending_bytes = raw_bytes
                .len()
                .saturating_add(stream_name.len())
                .saturating_add(ARTIFACT_FRAME_MAX_OVERHEAD_BYTES);
            if let Ok(mut artifact) = self.artifact.lock() {
                let archived_bytes = self.artifact_bytes.load(Ordering::Acquire);
                if archived_bytes.saturating_add(pending_bytes) > self.max_artifact_bytes {
                    self.artifact_truncated.store(true, Ordering::Release);
                } else if writeln!(artifact, "\n----- {stream_name} seq={sequence} -----")
                    .and_then(|_| artifact.write_all(raw_bytes))
                    .and_then(|_| artifact.flush())
                    .is_err()
                {
                    self.artifact_failed.store(true, Ordering::Release);
                } else {
                    self.artifact_bytes.store(
                        archived_bytes.saturating_add(pending_bytes),
                        Ordering::Release,
                    );
                }
            } else {
                self.artifact_failed.store(true, Ordering::Release);
            }
        }
        let state = match stream {
            ShellCommandOutputStream::Stdout => &self.stdout,
            ShellCommandOutputStream::Stderr => &self.stderr,
        };
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.receipt.append(display_bytes);
        drop(state);
        if !display_bytes.is_empty() {
            self.append_event(ShellCommandOutputChunk {
                stream,
                seq: sequence,
                data: display_bytes.to_vec(),
            });
        }
        self.notify_change();
    }

    pub(crate) fn take_receipt(&self, max_output_tokens: Option<usize>) -> ProcessOutputBatch {
        let mut stdout = self
            .stdout
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stderr = self
            .stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (stdout_budget, stderr_budget) = output_byte_budgets(
            max_output_tokens,
            stdout.receipt.total_bytes,
            stderr.receipt.total_bytes,
        );
        ProcessOutputBatch {
            stdout: stdout.receipt.take(stdout_budget).into_receipt(),
            stderr: stderr.receipt.take(stderr_budget).into_receipt(),
        }
    }

    pub(crate) fn take_events(&self) -> Vec<ShellCommandOutputChunk> {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let chunks = events.drain(..).collect::<Vec<_>>();
        *self
            .event_bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
        chunks
    }

    pub(crate) fn artifact_failed(&self) -> bool {
        self.artifact_failed.load(Ordering::Acquire)
    }

    /// 归档是否因达到容量上限而不再完整。
    pub(crate) fn artifact_truncated(&self) -> bool {
        self.artifact_truncated.load(Ordering::Acquire)
    }

    /// 已计入的归档字节数（含帧头预留，为**保守上界**）。
    ///
    /// 仅用于测试断言并发下计数未丢失：正确实现下它恒 ≥ 归档文件实际大小。
    #[cfg(test)]
    pub(crate) fn archived_bytes(&self) -> usize {
        self.artifact_bytes.load(Ordering::Acquire)
    }

    pub(crate) fn change_generation(&self) -> u64 {
        self.change.generation()
    }

    pub(crate) fn wait_for_change(&self, observed: u64, timeout: Duration) {
        self.change.wait(observed, timeout);
    }

    pub(crate) fn notify_change(&self) {
        self.change.notify();
    }

    fn append_event(&self, chunk: ShellCommandOutputChunk) {
        const MAX_PENDING_EVENT_BYTES: usize = OUTPUT_HEAD_BYTES + OUTPUT_TAIL_BYTES;

        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut event_bytes = self
            .event_bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *event_bytes = event_bytes.saturating_add(chunk.data.len());
        events.push_back(chunk);
        while *event_bytes > MAX_PENDING_EVENT_BYTES {
            let Some(removed) = events.pop_front() else {
                break;
            };
            *event_bytes = event_bytes.saturating_sub(removed.data.len());
        }
    }
}

#[derive(Default)]
struct ProcessOutputChange {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl ProcessOutputChange {
    fn generation(&self) -> u64 {
        *self
            .generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn notify(&self) {
        let mut generation = self
            .generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *generation = generation.wrapping_add(1);
        self.changed.notify_all();
    }

    fn wait(&self, observed: u64, timeout: Duration) {
        let generation = self
            .generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(
            self.changed
                .wait_timeout_while(generation, timeout, |current| *current == observed)
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
}

pub(crate) struct ProcessOutputBatch {
    pub(crate) stdout: ShellCommandBoundedStream,
    pub(crate) stderr: ShellCommandBoundedStream,
}

#[derive(Default)]
struct StreamState {
    receipt: OutputAccumulator,
}

#[derive(Default)]
struct OutputAccumulator {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total_bytes: usize,
    original_chars: usize,
    hasher: Sha256Hex,
}

impl OutputAccumulator {
    fn append(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        self.original_chars = self
            .original_chars
            .saturating_add(String::from_utf8_lossy(bytes).chars().count());
        self.hasher.update(bytes);
        let head_remaining = OUTPUT_HEAD_BYTES.saturating_sub(self.head.len());
        let (head, tail) = bytes.split_at(head_remaining.min(bytes.len()));
        self.head.extend_from_slice(head);
        // 批量维护尾部窗口，语义与「逐字节 pop_front + push_back」完全一致
        // （结果为「旧尾 ++ 新尾」的最后 OUTPUT_TAIL_BYTES 字节），但把每次追加的
        // 循环次数从 O(输入长度) 降为 O(被淘汰字节数)。
        //
        // 逐字节版本是输出热路径的主要热点：独立基准实测 64 MiB 输出为
        // 180.4 ms，等价批量实现为 33.6 ms（约 5.4×）。
        if tail.len() >= OUTPUT_TAIL_BYTES {
            // 新数据自身就填满窗口：只需保留其末尾 OUTPUT_TAIL_BYTES 字节。
            self.tail.clear();
            self.tail
                .extend(tail[tail.len() - OUTPUT_TAIL_BYTES..].iter().copied());
        } else {
            let overflow = self
                .tail
                .len()
                .saturating_add(tail.len())
                .saturating_sub(OUTPUT_TAIL_BYTES);
            if overflow > 0 {
                let _ = self.tail.drain(..overflow);
            }
            self.tail.extend(tail.iter().copied());
        }
    }

    fn take(&mut self, max_preview_bytes: Option<usize>) -> OutputCapture {
        let accumulator = std::mem::take(self);
        let mut stored = accumulator.head;
        stored.extend(accumulator.tail);
        let truncated = accumulator.total_bytes > stored.len()
            || max_preview_bytes.is_some_and(|budget| accumulator.total_bytes > budget);
        let preview = if truncated {
            bounded_preview(stored, max_preview_bytes)
        } else {
            stored
        };
        let retained_chars = String::from_utf8_lossy(&preview).chars().count();
        OutputCapture {
            preview,
            truncated,
            original_chars: accumulator.original_chars,
            retained_chars,
            sha256: accumulator.hasher.finalize_hex(),
        }
    }
}

fn output_byte_budgets(
    max_output_tokens: Option<usize>,
    stdout_bytes: usize,
    stderr_bytes: usize,
) -> (Option<usize>, Option<usize>) {
    let Some(max_output_tokens) = max_output_tokens else {
        return (None, None);
    };
    let budget = max_output_tokens.saturating_mul(APPROX_CHARS_PER_TOKEN);
    let total = stdout_bytes.saturating_add(stderr_bytes);
    if total <= budget || total == 0 {
        return (None, None);
    }
    let stdout_budget = budget.saturating_mul(stdout_bytes) / total;
    (
        Some(stdout_budget),
        Some(budget.saturating_sub(stdout_budget)),
    )
}

fn bounded_preview(stored: Vec<u8>, max_preview_bytes: Option<usize>) -> Vec<u8> {
    let marker_allowed =
        max_preview_bytes.is_none_or(|budget| budget >= OUTPUT_OMITTED_MARKER.len());
    let content_budget = max_preview_bytes.map_or(stored.len(), |budget| {
        if marker_allowed {
            budget.saturating_sub(OUTPUT_OMITTED_MARKER.len())
        } else {
            budget
        }
    });
    let requested_head_len = content_budget.div_ceil(2).min(stored.len());
    let requested_tail_len = content_budget
        .saturating_sub(requested_head_len)
        .min(stored.len().saturating_sub(requested_head_len));
    let head_len = valid_utf8_prefix_len(&stored, requested_head_len);
    let tail_start =
        valid_utf8_suffix_start(&stored, stored.len().saturating_sub(requested_tail_len));
    let tail_len = stored.len().saturating_sub(tail_start);
    let mut preview = Vec::with_capacity(
        head_len
            .saturating_add(tail_len)
            .saturating_add(OUTPUT_OMITTED_MARKER.len()),
    );
    preview.extend_from_slice(&stored[..head_len]);
    if marker_allowed {
        preview.extend_from_slice(OUTPUT_OMITTED_MARKER);
    }
    if tail_len > 0 {
        preview.extend_from_slice(&stored[tail_start..]);
    }
    preview
}

fn valid_utf8_prefix_len(bytes: &[u8], requested: usize) -> usize {
    std::str::from_utf8(&bytes[..requested]).map_or_else(|error| error.valid_up_to(), |_| requested)
}

fn valid_utf8_suffix_start(bytes: &[u8], requested: usize) -> usize {
    let mut start = requested;
    while start < bytes.len() && bytes[start] & 0b1100_0000 == 0b1000_0000 {
        start += 1;
    }
    start
}

struct OutputCapture {
    preview: Vec<u8>,
    truncated: bool,
    original_chars: usize,
    retained_chars: usize,
    sha256: String,
}

impl OutputCapture {
    fn into_receipt(self) -> ShellCommandBoundedStream {
        ShellCommandBoundedStream::from_capture(
            String::from_utf8_lossy(&self.preview).into_owned(),
            self.truncated,
            self.original_chars,
            self.retained_chars,
            self.sha256,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_preview_keeps_utf8_character_boundaries() {
        let mut source = vec![b'a'; OUTPUT_HEAD_BYTES - 1];
        source.extend_from_slice("中".as_bytes());
        source.extend(std::iter::repeat_n(b'b', OUTPUT_TAIL_BYTES + 128));
        let mut accumulator = OutputAccumulator::default();

        accumulator.append(&source);
        let capture = accumulator.take(None);

        assert!(capture.truncated);
        assert!(String::from_utf8(capture.preview).is_ok());
    }

    /// P2 回归：批量维护尾部窗口必须与「逐字节 pop_front + push_back」语义等价。
    ///
    /// 逐字节版本是输出热路径的主要热点（独立基准实测 64 MiB 为 180.4 ms，
    /// 等价批量实现 33.6 ms）。本用例以旧实现为参考基准逐窗口比对，
    /// 覆盖「小于窗口 / 累积溢出 / 单次超窗 / 恰好等于窗口 / 含空块 / 超头部窗口」六种边界。
    #[test]
    fn batched_tail_window_matches_the_append_only_semantics() {
        /// 参考实现：P2 修复前的逐字节语义。
        fn reference(chunks: &[Vec<u8>]) -> (Vec<u8>, Vec<u8>) {
            let mut head = Vec::new();
            let mut tail = std::collections::VecDeque::new();
            for bytes in chunks {
                let head_remaining = OUTPUT_HEAD_BYTES.saturating_sub(head.len());
                let (head_part, tail_part) = bytes.split_at(head_remaining.min(bytes.len()));
                head.extend_from_slice(head_part);
                for byte in tail_part {
                    if tail.len() >= OUTPUT_TAIL_BYTES {
                        tail.pop_front();
                    }
                    tail.push_back(*byte);
                }
            }
            (head, tail.into_iter().collect())
        }

        let cases: Vec<Vec<Vec<u8>>> = vec![
            vec![vec![b'a'; 64], vec![b'b'; 64]],
            vec![
                vec![b'1'; OUTPUT_TAIL_BYTES / 2],
                vec![b'2'; OUTPUT_TAIL_BYTES],
                vec![b'3'; OUTPUT_TAIL_BYTES / 2],
            ],
            vec![vec![b'x'; OUTPUT_TAIL_BYTES * 3]],
            vec![vec![b'y'; OUTPUT_TAIL_BYTES]],
            vec![Vec::new(), vec![b'z'; 8], Vec::new()],
            vec![vec![b'h'; OUTPUT_HEAD_BYTES * 2], vec![b't'; 32]],
        ];

        for (index, chunks) in cases.iter().enumerate() {
            let mut accumulator = OutputAccumulator::default();
            for chunk in chunks {
                accumulator.append(chunk);
            }
            let (expected_head, expected_tail) = reference(chunks);

            assert_eq!(
                accumulator.head, expected_head,
                "用例 {index}：头部窗口不一致"
            );
            assert_eq!(
                accumulator.tail.iter().copied().collect::<Vec<_>>(),
                expected_tail,
                "用例 {index}：尾部窗口必须与逐字节语义一致"
            );
        }
    }
}
