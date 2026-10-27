//! `A-10` 只读并行基准：4 路只读、每路 150ms、5 轮取中位数。
//!
//! 夹具形状与 `limits.yaml` 的 `benchmark.read_units` / `read_delay_ms` / `rounds`
//! 一致；串行与并行跑同一批任务，只比较墙钟。token 增幅在本阶段恒为 0
//! （阶段 1 不改提示词），由调用方登记。

use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use super::run_capacity::CapacitySnapshot;
use super::run_tool_pool::session_tool_pool;

const READ_UNITS: usize = 4;
const READ_DELAY_MS: u64 = 150;
const ROUNDS: usize = 5;
const POOL_SESSION: &str = "a10-read-parallel-benchmark";

/// 跑一批只读任务：`concurrent=false` 时一个接一个等完成。
fn run_batch(concurrent: bool) -> Duration {
    let pool = session_tool_pool(POOL_SESSION, CapacitySnapshot::preset());
    let started = Instant::now();
    if concurrent {
        let (tx, rx) = mpsc::channel();
        for _ in 0..READ_UNITS {
            let tx = tx.clone();
            pool.submit(move || {
                thread::sleep(Duration::from_millis(READ_DELAY_MS));
                let _ = tx.send(());
            })
            .expect("池可用时投递必须成功");
        }
        drop(tx);
        for _ in 0..READ_UNITS {
            rx.recv().expect("并行任务必须完成");
        }
    } else {
        for _ in 0..READ_UNITS {
            let (tx, rx) = mpsc::channel();
            pool.submit(move || {
                thread::sleep(Duration::from_millis(READ_DELAY_MS));
                let _ = tx.send(());
            })
            .expect("池可用时投递必须成功");
            rx.recv().expect("串行任务必须完成");
        }
    }
    started.elapsed()
}

fn median(samples: &[Duration]) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// 用与基准相同的 4 个只读调用跑一次切段，返回段区间。
fn scheduled_segments() -> Vec<std::ops::Range<usize>> {
    let cwd = std::path::Path::new(".");
    let scopes = (0..READ_UNITS)
        .map(|index| {
            let request = crate::tool_orchestrator::ToolExecutionRequest {
                tool_call: axon_agent_runtime::ProviderToolCall::new(
                    format!("bench-{index}"),
                    "builtin.memory_search",
                    "{}",
                ),
                tool_call_id: axon_domain::ToolCallId(index as u64 + 1),
                part_id: format!("tool-part:bench:{index}"),
                execution_request_id: axon_domain::ExecutionRequestId(index as u64 + 1),
                kind: crate::tool_orchestrator::ToolKind::MemorySearch,
                concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
                execution_permission: axon_domain::ToolExecutionPermission::Allowed,
            };
            super::run_tool_write_coordination::tool_access_scope(cwd, &request)
        })
        .collect::<Vec<_>>();
    super::run_tool_loop::tool_batch_ranges(&scopes)
}

/// `A-10`：墙钟下降 ≥ 0.5，token 增幅 ≤ 0.3（本阶段为 0）。
#[test]
fn read_parallel_reduces_wall_clock_by_at_least_half() {
    // 先证明调度层确实把这批只读调用编进同一段，基准不是只测池。
    assert_eq!(
        scheduled_segments(),
        vec![0..READ_UNITS],
        "4 个只读调用必须落在同一个并发段"
    );
    let mut serial = Vec::with_capacity(ROUNDS);
    let mut parallel = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        serial.push(run_batch(false));
        parallel.push(run_batch(true));
    }
    let serial_ms = median(&serial).as_millis();
    let parallel_ms = median(&parallel).as_millis();
    let reduction = 1.0 - parallel_ms as f64 / serial_ms as f64;
    let report = serde_json::json!({
        "gate": "A-10",
        "units": READ_UNITS,
        "delay_ms": READ_DELAY_MS,
        "rounds": ROUNDS,
        "serial_ms": serial_ms,
        "parallel_ms": parallel_ms,
        "wall_time_reduction": (reduction * 1000.0).round() / 1000.0,
        "token_overhead": 0.0,
        "note": "阶段 1 不改提示词，token 增幅恒为 0"
    });
    println!("A10_REPORT {}", report);
    assert!(
        reduction >= 0.5,
        "只读并行墙钟下降不足：serial={serial_ms}ms parallel={parallel_ms}ms"
    );
}
