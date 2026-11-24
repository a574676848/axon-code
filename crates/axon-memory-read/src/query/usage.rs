use std::sync::{OnceLock, mpsc};

use axon_domain::AxonResult;
use axon_memory::{
    MemoryCitation, MemoryHybridHit, MemoryOrchestrator, MemoryUsageEvent, MemoryUsageKind,
    MemoryUsageStore,
};

/// 记录 Memory 读使用信号。
pub fn record_read_usage(
    orchestrator: &MemoryOrchestrator,
    events: &[MemoryUsageEvent],
) -> AxonResult<()> {
    let store = MemoryUsageStore::open(orchestrator.config())?;
    store.record_events(events)
}

#[derive(Debug)]
struct MemoryUsageWriteJob {
    orchestrator: MemoryOrchestrator,
    events: Vec<MemoryUsageEvent>,
}

/// 把 Memory 读使用信号提交给进程级单 worker，移出 Provider dispatch 关键路径。
pub fn record_read_usage_async(
    orchestrator: MemoryOrchestrator,
    events: Vec<MemoryUsageEvent>,
) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    memory_usage_sender()
        .send(MemoryUsageWriteJob {
            orchestrator,
            events,
        })
        .map_err(|_| "Memory usage 异步写入队列不可用".to_owned())
}

fn memory_usage_sender() -> &'static mpsc::Sender<MemoryUsageWriteJob> {
    static SENDER: OnceLock<mpsc::Sender<MemoryUsageWriteJob>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<MemoryUsageWriteJob>();
        std::thread::Builder::new()
            .name("axon-memory-usage".to_owned())
            .spawn(move || memory_usage_worker(&receiver))
            .expect("Memory usage worker 应可启动");
        sender
    })
}

fn memory_usage_worker(receiver: &mpsc::Receiver<MemoryUsageWriteJob>) {
    while let Ok(first) = receiver.recv() {
        let mut jobs = vec![first];
        jobs.extend(receiver.try_iter());
        while let Some(job) = jobs.pop() {
            let mut events = job.events;
            let mut index = 0;
            while index < jobs.len() {
                if jobs[index].orchestrator.config() == job.orchestrator.config() {
                    events.extend(jobs.swap_remove(index).events);
                } else {
                    index += 1;
                }
            }
            let _ = record_read_usage(&job.orchestrator, &events);
        }
    }
}

/// 将 hit 转换为 recall usage 事件。
#[must_use]
pub fn recalled_usage_events(hits: &[MemoryHybridHit], now_ms: u64) -> Vec<MemoryUsageEvent> {
    hits.iter()
        .map(|hit| MemoryUsageEvent {
            memory_id: hit.record.record.id,
            kind: MemoryUsageKind::Recalled,
            occurred_at_ms: now_ms,
        })
        .collect()
}

/// 将 hit 转换为已注入 usage 事件。
#[must_use]
pub fn injected_usage_events(hits: &[MemoryHybridHit], now_ms: u64) -> Vec<MemoryUsageEvent> {
    hits.iter()
        .map(|hit| MemoryUsageEvent {
            memory_id: hit.record.record.id,
            kind: MemoryUsageKind::Injected,
            occurred_at_ms: now_ms,
        })
        .collect()
}

/// 将 citation 转成 usage 事件。
#[must_use]
pub fn cited_usage_events(citations: &[MemoryCitation], now_ms: u64) -> Vec<MemoryUsageEvent> {
    citations
        .iter()
        .map(|citation| MemoryUsageEvent {
            memory_id: citation.memory_id,
            kind: MemoryUsageKind::Cited,
            occurred_at_ms: now_ms,
        })
        .collect()
}
