//! 并行调度测试缝：可注入延迟与真实取消令牌，复用生产池与提交游标。
//!
//! 只用于门禁证据；生产路径仍走 `execute_pooled_tool`。

use std::{sync::mpsc, time::Duration};

use axon_agent_runtime::{RunCancelToken, agent_run_failed_tool_receipt};

use super::{
    CapacitySnapshot, ParallelToolProgress, RunToolReceipt, TimedToolReceipt, ToolExecutionRequest,
    commit_ready_receipts, fill_unstarted_receipts, session_tool_pool,
};

/// 测试缝的池名序列：每个用例独占一个池。
static SEAM_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 生成本次调用独占的池会话名。
///
/// 池按会话名全局缓存，若所有用例共用同一个固定名字，就会共享同一批 worker：
/// 「有多少调用在排队」于是取决于别的用例此刻占用了多少 worker，排队类断言随之随机翻车。
fn seam_session() -> String {
    let sequence = SEAM_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("parallel-seam-test-{sequence}-{nanos}")
}

/// 跑一批假任务：延迟来自 `latency_ms`，取消令牌与生产同构（提交前与任务启动前各查一次）。
pub(crate) fn execute_parallel_for_tests(
    execution_requests: &[ToolExecutionRequest],
    latency_ms: &[u64],
    cancel: Option<RunCancelToken>,
    commit_log: &mut Vec<usize>,
) -> Vec<RunToolReceipt> {
    let total = execution_requests.len();
    let pool = session_tool_pool(&seam_session(), CapacitySnapshot::preset());
    let (event_tx, event_rx) = mpsc::channel();
    let mut started = 0usize;
    for (index, execution_request) in execution_requests.iter().enumerate() {
        if cancel
            .as_ref()
            .is_some_and(axon_agent_runtime::RunCancelToken::is_cancelled)
        {
            break;
        }
        started += 1;
        let event_tx = event_tx.clone();
        let delay = latency_ms.get(index).copied().unwrap_or(0);
        let call = execution_request.tool_call.clone();
        let part_id = execution_request.part_id.clone();
        let call_id = execution_request.tool_call_id.0;
        let task_cancel = cancel.clone();
        pool.submit(move || {
            // 与 `execute_pooled_tool` 同构：启动前查一次取消。
            let cancelled = task_cancel
                .as_ref()
                .is_some_and(axon_agent_runtime::RunCancelToken::is_cancelled);
            if !cancelled {
                std::thread::sleep(Duration::from_millis(delay));
            }
            let mut receipt = crate::tool_orchestrator::tool_receipt_from_runtime(
                agent_run_failed_tool_receipt(&call, if cancelled { "未启动" } else { "seam" }),
            );
            receipt.tool_call_id = call_id;
            receipt.part_id = part_id;
            let _ = event_tx.send(ParallelToolProgress::Finished {
                index,
                receipt: TimedToolReceipt {
                    receipt,
                    duration_ms: Some(delay),
                },
            });
        })
        .expect("测试池应接受任务");
    }
    drop(event_tx);

    let mut slots: Vec<Option<TimedToolReceipt>> = (0..total).map(|_| None).collect();
    let mut receipts: Vec<Option<RunToolReceipt>> = (0..total).map(|_| None).collect();
    let mut committed = 0usize;
    let mut finished = 0usize;
    while finished < started {
        let Ok(ParallelToolProgress::Finished { index, receipt }) = event_rx.recv() else {
            break;
        };
        finished += 1;
        slots[index] = Some(receipt);
        commit_ready_receipts(
            &mut slots,
            &mut receipts,
            &mut committed,
            execution_requests,
            None,
            &mut |index| commit_log.push(index),
        );
    }
    fill_unstarted_receipts(
        &mut slots,
        &mut receipts,
        committed,
        execution_requests,
        None,
    );
    receipts.into_iter().flatten().collect()
}
