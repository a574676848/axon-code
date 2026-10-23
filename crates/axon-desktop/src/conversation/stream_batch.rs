//! 会话事件的下行攒批。
//!
//! 历史回放一次可能产生上万帧。逐帧调用 `AppHandle::emit` 等于逐帧往 WebView 里
//! 注入一段脚本：投递频率越高，WebView 侧越容易跟不上，而**丢掉的帧不会重来**，
//! 界面就永远停在「生成中」。因此把投递改成按窗口合并：窗口内的帧按原顺序装进
//! 一个批量载荷一次送出，频率从「每帧一次」降到「每 16ms 一次」。
//!
//! 顺序不变量：批量只做合并、不做重排。所有生产者共用一个队列，单个消费者按
//! 入队顺序取出并成批，因此前端按批展开后看到的顺序与逐帧投递完全一致。

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::wire::StreamEvent;

/// 攒批窗口：一个显示帧的时长。再短没有意义，再长会让流式输出显出顿挫。
pub const STREAM_BATCH_WINDOW: Duration = Duration::from_millis(16);
/// 单批帧数上限：超出就先送一批，避免一次序列化过重。
pub const STREAM_BATCH_LIMIT: usize = 512;

/// 批量载荷：一次投递里按顺序装着的若干帧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamEventBatch {
    /// 本批的帧，顺序与产生顺序一致。
    pub batch: Vec<StreamEvent>,
}

impl StreamEventBatch {
    /// 装一批帧。
    #[must_use]
    pub fn new(batch: Vec<StreamEvent>) -> Self {
        Self { batch }
    }
}

/// 攒批投递句柄。
///
/// 事件生产者在任意线程上调用 [`push`](Self::push)，由后台线程按窗口合并后交给
/// `emit`。句柄被丢弃不会丢帧：队列关闭后消费者把剩下的帧送完再退出。
#[derive(Clone)]
pub struct StreamBatcher {
    sender: Sender<StreamEvent>,
}

impl StreamBatcher {
    /// 启动一个攒批投递器。
    pub fn start(emit: Arc<dyn Fn(StreamEventBatch) + Send + Sync>) -> Self {
        Self::start_with(emit, STREAM_BATCH_WINDOW, STREAM_BATCH_LIMIT)
    }

    /// 以指定窗口与批量上限启动；窗口与上限对测试可见，生产只用默认值。
    pub fn start_with(
        emit: Arc<dyn Fn(StreamEventBatch) + Send + Sync>,
        window: Duration,
        limit: usize,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let limit = limit.max(1);
        // 投递线程不持有任何界面句柄：`emit` 由调用方决定打到哪条通道，测试因此
        // 可以直接收集批次而不需要宿主运行时。
        let _ = thread::Builder::new()
            .name("axon-stream-batcher".to_owned())
            .spawn(move || drain(receiver, emit, window, limit));
        Self { sender }
    }

    /// 入队一帧；投递线程已退出时静默丢弃（此时进程正在收口）。
    pub fn push(&self, event: StreamEvent) {
        let _ = self.sender.send(event);
    }
}

/// 队列消费循环：取到第一帧后最多等一个窗口，凑满上限就立刻送出。
fn drain(
    receiver: mpsc::Receiver<StreamEvent>,
    emit: Arc<dyn Fn(StreamEventBatch) + Send + Sync>,
    window: Duration,
    limit: usize,
) {
    loop {
        let Ok(first) = receiver.recv() else {
            return;
        };
        let mut batch = Vec::with_capacity(limit.min(64));
        batch.push(first);
        let deadline = Instant::now() + window;
        while batch.len() < limit {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match receiver.recv_timeout(remaining) {
                Ok(next) => batch.push(next),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    emit(StreamEventBatch::new(batch));
                    return;
                }
            }
        }
        emit(StreamEventBatch::new(batch));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn frame(sequence: usize) -> StreamEvent {
        StreamEvent::Frame {
            subscribe_id: "interaction:test".to_owned(),
            event_type: "wire_event".to_owned(),
            payload_json: sequence.to_string(),
            cursor: None,
            replay: true,
        }
    }

    fn sequence_of(event: &StreamEvent) -> usize {
        match event {
            StreamEvent::Frame { payload_json, .. } => payload_json.parse().expect("帧序号"),
            _ => panic!("测试只产生事件帧"),
        }
    }

    /// 收集到的批次：每个元素是一批帧。
    type CollectedBatches = Arc<Mutex<Vec<Vec<StreamEvent>>>>;
    /// 测试用的投递接收端。
    type TestSink = Arc<dyn Fn(StreamEventBatch) + Send + Sync>;

    /// 收集所有批次，供测试断言「批内顺序」与「批数」。
    fn collector() -> (CollectedBatches, TestSink) {
        let collected = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&collected);
        let emit: Arc<dyn Fn(StreamEventBatch) + Send + Sync> = Arc::new(move |batch| {
            sink.lock().expect("批次表").push(batch.batch);
        });
        (collected, emit)
    }

    fn wait_for_batches(collected: &Arc<Mutex<Vec<Vec<StreamEvent>>>>, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if collected.lock().expect("批次表").len() >= expected {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "等待 {expected} 个批次超时，实际 {}",
            collected.lock().expect("批次表").len()
        );
    }

    #[test]
    fn one_window_coalesces_frames_in_production_order() {
        let (collected, emit) = collector();
        let batcher = StreamBatcher::start_with(emit, Duration::from_millis(40), 512);
        for index in 0..100 {
            batcher.push(frame(index));
        }
        wait_for_batches(&collected, 1);
        // 窗口内连发 100 帧必须落在同一批，且顺序不被打乱。
        let batches = collected.lock().expect("批次表").clone();
        assert_eq!(batches.len(), 1, "一个窗口只应产生一批");
        assert_eq!(batches[0].len(), 100);
        let order: Vec<usize> = batches[0].iter().map(sequence_of).collect();
        assert_eq!(order, (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn a_batch_never_exceeds_the_limit() {
        let (collected, emit) = collector();
        let batcher = StreamBatcher::start_with(emit, Duration::from_millis(40), 10);
        for index in 0..25 {
            batcher.push(frame(index));
        }
        wait_for_batches(&collected, 3);
        let batches = collected.lock().expect("批次表").clone();
        assert!(batches.iter().all(|batch| batch.len() <= 10), "{batches:?}");
        let flat: Vec<usize> = batches.iter().flatten().map(sequence_of).collect();
        assert_eq!(flat, (0..25).collect::<Vec<_>>(), "分多批也要保持全局顺序");
    }

    #[test]
    fn frames_arriving_after_the_window_start_a_new_batch() {
        let (collected, emit) = collector();
        let batcher = StreamBatcher::start_with(emit, Duration::from_millis(30), 512);
        batcher.push(frame(1));
        wait_for_batches(&collected, 1);
        thread::sleep(Duration::from_millis(60));
        batcher.push(frame(2));
        wait_for_batches(&collected, 2);
        let batches = collected.lock().expect("批次表").clone();
        assert_eq!(batches.len(), 2, "窗口关闭后的帧属于下一批");
        assert_eq!(sequence_of(&batches[1][0]), 2);
    }
}
