//! `G1` 只读并行基准：4 路只读、每路 150ms、5 轮取中位数。
//!
//! 基准走真实调度缝（池 + 切段 + 提交游标），不是裸线程池计时；
//! 串行与并行使用同一批调用，只比较墙钟。token 增幅在 PAR-1 恒为 0。

use std::time::{Duration, Instant};

use super::run_capacity::CapacitySnapshot;

const READ_UNITS: usize = 4;
const READ_DELAY_MS: u64 = 150;
const ROUNDS: usize = 5;

/// 构造与基准一致的 4 个只读调用。
fn bench_requests() -> Vec<crate::tool_orchestrator::ToolExecutionRequest> {
    (0..READ_UNITS)
        .map(|index| crate::tool_orchestrator::ToolExecutionRequest {
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
        })
        .collect()
}

/// 同一批调用在调度层必须编进同一个并发段。
fn scheduled_segments() -> Vec<std::ops::Range<usize>> {
    let cwd = std::path::Path::new(".");
    let requests = bench_requests();
    let scopes = requests
        .iter()
        .map(|request| super::run_tool_write_coordination::tool_access_scope(cwd, request))
        .collect::<Vec<_>>();
    super::run_tool_loop::tool_batch_ranges(&scopes)
}

fn median(samples: &[Duration]) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// 并行：整批一次提交；串行：逐个调用单独提交。
fn run_batch(
    requests: &[crate::tool_orchestrator::ToolExecutionRequest],
    concurrent: bool,
) -> Duration {
    let started = Instant::now();
    let mut commit_log = Vec::new();
    if concurrent {
        let latencies = vec![READ_DELAY_MS; requests.len()];
        let _ = super::run_tool_loop::parallel_test_seam::execute_parallel_for_tests(
            requests,
            &latencies,
            None,
            &mut commit_log,
        );
    } else {
        for request in requests {
            let _ = super::run_tool_loop::parallel_test_seam::execute_parallel_for_tests(
                std::slice::from_ref(request),
                &[READ_DELAY_MS],
                None,
                &mut commit_log,
            );
        }
    }
    started.elapsed()
}

/// `G1`：墙钟下降 ≥ 0.5。
#[test]
fn read_parallel_reduces_wall_clock_by_at_least_half() {
    let requests = bench_requests();
    let _ = CapacitySnapshot::preset();
    assert_eq!(
        scheduled_segments(),
        vec![0..READ_UNITS],
        "4 个只读调用必须落在同一个并发段"
    );

    let mut serial = Vec::with_capacity(ROUNDS);
    let mut parallel = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        serial.push(run_batch(&requests, false));
        parallel.push(run_batch(&requests, true));
    }
    let serial_ms = median(&serial).as_millis();
    let parallel_ms = median(&parallel).as_millis();
    let reduction = 1.0 - parallel_ms as f64 / serial_ms as f64;
    let report = serde_json::json!({
        "gate": "G1",
        "units": READ_UNITS,
        "delay_ms": READ_DELAY_MS,
        "rounds": ROUNDS,
        "serial_ms": serial_ms,
        "parallel_ms": parallel_ms,
        "wall_time_reduction": (reduction * 1000.0).round() / 1000.0,
        "token_overhead": 0.0,
        "path": "execute_parallel_for_tests（池 + 切段 + 提交游标）"
    });
    println!("G1_REPORT {}", report);
    assert!(
        reduction >= 0.5,
        "只读并行墙钟下降不足：serial={serial_ms}ms parallel={parallel_ms}ms"
    );
}
