use std::{
    collections::HashSet,
    ops::Range,
    panic::AssertUnwindSafe,
    sync::{Arc, mpsc},
    time::Instant,
};

use axon_agent_runtime::{ProviderToolCall, agent_run_failed_tool_receipt};
use axon_domain::{CellId, SessionId, ToolConcurrencyPolicy};

use super::{
    RunRequest,
    run_capacity::CapacitySnapshot,
    run_progress::RunProgressSink,
    run_tool_pool::session_tool_pool,
    run_tool_result_cache::execute_tool_call_with_cache,
    run_tool_write_coordination::{
        ToolAccessScope, WorkspaceWriteCoordinator, tool_access_scope, workspace_write_coordinator,
    },
};
use crate::stable_ids::stable_numeric_id;
#[cfg(test)]
use crate::tool_orchestrator::NON_PROVIDER_TURN_ID;
use crate::tool_orchestrator::{
    ToolExecutionRequest, ToolOrchestratorContext, ToolProgressInfo, ToolReceipt, ToolRuntimeEvent,
    failed_tool_schedule_receipts, schedule_tool_calls,
};

pub(super) type RunToolReceipt = ToolReceipt;

/// 并行工具 worker 的完成回执：除了工具回执，还带回本次执行的实测耗时，
/// 供协调线程在 worker 线程之外投影进度事件。
struct TimedToolReceipt {
    receipt: RunToolReceipt,
    duration_ms: Option<u64>,
}

#[cfg(test)]
pub(super) fn execute_supported_tool_calls(
    request: &RunRequest,
    tool_calls: &[ProviderToolCall],
    progress: Option<&RunProgressSink<'_>>,
) -> Vec<RunToolReceipt> {
    execute_supported_tool_calls_for_turn(request, tool_calls, NON_PROVIDER_TURN_ID, progress)
}

pub(super) fn execute_supported_tool_calls_for_turn(
    request: &RunRequest,
    tool_calls: &[ProviderToolCall],
    provider_turn_id: u64,
    progress: Option<&RunProgressSink<'_>>,
) -> Vec<RunToolReceipt> {
    request
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .begin_spawn_intent_turn(request.current_agent_path.as_str());
    let context = ToolOrchestratorContext {
        config_path: Some(request.config_path.clone()),
        cwd: request.cwd.clone(),
        session_id: SessionId(stable_numeric_id(&request.session_id)),
        thread_session_id: request.session_id.clone(),
        cell_id: CellId(1),
        model_messages: request.model_payload.messages.clone(),
        execution_tools: Arc::clone(&request.execution_tools),
        memory_write_runtime: Arc::clone(&request.startup.memory_write_runtime),
        workspace_files: Arc::clone(&request.workspace_files),
        timeout: request.timeout,
        goal_disclosure_policy: request.goal_disclosure_policy,
        goal_transport_context: request.goal_transport_context.clone(),
        cancel_token: request.cancel_token.as_ref().map(|token| {
            let token = token.clone();
            Arc::new(move || token.is_cancelled()) as crate::tool_orchestrator::ToolCancelToken
        }),
        run_cancel_token: request.cancel_token.clone(),
        model_profile_id: request.model_profile_id,
        permission_snapshot_id: request.permission_snapshot_id,
        current_agent_path: request.current_agent_path.clone(),
        agent_control: request.agent_control.clone(),
        agent_graph_store: request.agent_graph_store.clone(),
        subagent_sessions: request.subagent_sessions.clone(),
        subagent_executor: request.subagent_executor.clone(),
        subagent_cancel_tokens: request.subagent_cancel_tokens.clone(),
        user_question_port: request.user_question_port.clone(),
    };
    let registry = request
        .execution_tools
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let execution_requests = match schedule_tool_calls(
        &registry,
        tool_calls,
        provider_turn_id,
        &context.cwd,
        context.session_id,
        context.cell_id,
        super::now_ms(),
    ) {
        Ok(execution_requests) => execution_requests,
        Err(error) => {
            return failed_tool_schedule_receipts(tool_calls, provider_turn_id, &error);
        }
    };
    // 执行域在启动前统一派生一次：切段、准入与并发协调读同一份结论。
    let scopes = execution_requests
        .iter()
        .map(|execution_request| tool_access_scope(&context.cwd, execution_request))
        .collect::<Vec<_>>();
    // 已取消也要为每个 call 生成配对回执，否则会话日志会留下孤立 tool_call。
    if run_cancelled(request) {
        return cancelled_receipts(&execution_requests, progress);
    }
    let capacity = CapacitySnapshot::preset();
    // 批次按读写交叉切成连续子段：段内可并发，段间严格顺序。
    // 独占请求自成一段，因此仍不会与同批其他工具重叠。
    let mut receipts = Vec::with_capacity(execution_requests.len());
    for range in tool_batch_ranges(&scopes) {
        let batch = &execution_requests[range.clone()];
        let batch_scopes = &scopes[range.clone()];
        if tool_calls_can_run_in_parallel(batch) {
            receipts.extend(execute_parallel_tool_calls(
                &context, batch, progress, capacity,
            ));
            continue;
        }
        for (execution_request, scope) in batch.iter().zip(batch_scopes) {
            receipts.push(execute_supported_tool_call(
                &context,
                request,
                execution_request,
                scope,
                progress,
            ));
        }
    }
    receipts
}

fn execute_supported_tool_call(
    context: &ToolOrchestratorContext,
    request: &RunRequest,
    execution_request: &ToolExecutionRequest,
    scope: &ToolAccessScope,
    progress: Option<&RunProgressSink<'_>>,
) -> RunToolReceipt {
    let tool_call = &execution_request.tool_call;
    if run_cancelled(request) {
        let mut receipt = crate::tool_orchestrator::tool_receipt_from_runtime(
            agent_run_failed_tool_receipt(tool_call, "任务已由用户中断，未启动该工具调用"),
        );
        attach_tool_call_id(&mut receipt, execution_request);
        emit_tool_completed(progress, execution_request, &receipt, None);
        return receipt;
    }
    if context.is_cancelled() {
        let mut receipt = cancelled_tool_receipt(tool_call);
        attach_tool_call_id(&mut receipt, execution_request);
        emit_tool_completed(progress, execution_request, &receipt, None);
        return receipt;
    }
    if let Some(sink) = progress {
        sink.tool_execution_started(execution_request);
    }
    let started_at = Instant::now();
    let mut observer = progress.map(|sink| {
        let sink = sink.clone();
        move |event| sink.tool_runtime_event(event)
    });
    let write_coordinator = workspace_write_coordinator(&context.cwd);
    let mut receipt = write_coordinator.execute(scope, || {
        if run_cancelled(request) || context.is_cancelled() {
            return cancelled_tool_receipt(tool_call);
        }
        execute_tool_call_with_cache(
            context,
            execution_request,
            observer.as_mut().map(|observer| observer as &mut _),
        )
    });
    attach_tool_call_id(&mut receipt, execution_request);
    // 工具已返回时保留真实回执：它可能已经写盘，替换成"已取消"会让模型重复执行。
    receipt.duration_ms = Some(elapsed_ms(started_at));
    if let Some(sink) = progress {
        sink.tool_execution_completed(execution_request, &receipt, receipt.duration_ms);
    }
    receipt
}

fn emit_tool_completed(
    progress: Option<&RunProgressSink<'_>>,
    execution_request: &ToolExecutionRequest,
    receipt: &RunToolReceipt,
    duration_ms: Option<u64>,
) {
    if let Some(sink) = progress {
        sink.tool_execution_completed(execution_request, receipt, duration_ms);
    }
}

fn attach_tool_call_id(receipt: &mut RunToolReceipt, execution_request: &ToolExecutionRequest) {
    receipt.tool_call_id = execution_request.tool_call_id.0;
    receipt.part_id.clone_from(&execution_request.part_id);
}

fn run_cancelled(request: &RunRequest) -> bool {
    request
        .cancel_token
        .as_ref()
        .is_some_and(super::RunCancelToken::is_cancelled)
}

/// 判定一个连续子段是否走并发执行。
///
/// 工具审批已全局关闭，真正要求独占的只有 Exclusive。
fn tool_calls_can_run_in_parallel(execution_requests: &[ToolExecutionRequest]) -> bool {
    if execution_requests.len() <= 1
        || execution_requests
            .iter()
            .any(|request| request.concurrency_policy == ToolConcurrencyPolicy::Exclusive)
    {
        return false;
    }
    execution_requests
        .iter()
        .filter(|request| request.kind == crate::tool_orchestrator::ToolKind::Subagent)
        .all(is_parallel_subagent_request)
        && spawn_agent_intents_are_unique(execution_requests)
}

fn spawn_agent_intents_are_unique(execution_requests: &[ToolExecutionRequest]) -> bool {
    let mut fingerprints = HashSet::with_capacity(execution_requests.len());
    execution_requests
        .iter()
        .filter_map(|request| {
            crate::tool_orchestrator::spawn_intent_fingerprint(&request.tool_call)
        })
        .all(|fingerprint| fingerprints.insert(fingerprint))
}

fn is_parallel_subagent_request(request: &ToolExecutionRequest) -> bool {
    request.kind == crate::tool_orchestrator::ToolKind::Subagent
        && matches!(
            request.tool_call.dispatch_name(),
            axon_agent_runtime::DELEGATE_TO_FAST_TOOL_NAME | "builtin.delegate_to_fast"
        )
}

fn execute_parallel_tool_calls(
    context: &ToolOrchestratorContext,
    execution_requests: &[ToolExecutionRequest],
    progress: Option<&RunProgressSink<'_>>,
    capacity: CapacitySnapshot,
) -> Vec<RunToolReceipt> {
    // `S-5`：启动前按最新参数重分类；因分类变化出现的新冲突，在未启动子集上重切。
    let fresh_scopes = execution_requests
        .iter()
        .map(|execution_request| tool_access_scope(&context.cwd, execution_request))
        .collect::<Vec<_>>();
    let fresh_ranges = tool_batch_ranges(&fresh_scopes);
    if fresh_ranges.len() > 1 {
        let mut receipts = Vec::with_capacity(execution_requests.len());
        for range in fresh_ranges {
            receipts.extend(execute_parallel_tool_calls(
                context,
                &execution_requests[range],
                progress,
                capacity,
            ));
        }
        return receipts;
    }
    let total = execution_requests.len();
    let context = Arc::new(context.clone());
    let write_coordinator = workspace_write_coordinator(&context.cwd);
    let pool = session_tool_pool(&context.thread_session_id, capacity);
    let (event_tx, event_rx) = mpsc::channel();
    let mut started = 0usize;
    for (index, execution_request) in execution_requests.iter().enumerate() {
        // 取消后不再启动新调用，未启动的调用稍后补合成回执。
        if is_run_cancelled(&context) {
            break;
        }
        let task_context = Arc::clone(&context);
        let task_request = Arc::new(execution_request.clone());
        let task_coordinator = Arc::clone(&write_coordinator);
        let task_events = event_tx.clone();
        let submitted = pool.submit(move || {
            let fallback_call = task_request.tool_call.clone();
            let fallback_part = task_request.part_id.clone();
            let fallback_id = task_request.tool_call_id.0;
            let receipt = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let _ = task_events.send(ParallelToolProgress::Started { index });
                execute_pooled_tool(
                    &task_context,
                    &task_request,
                    index,
                    total,
                    task_events.clone(),
                    &task_coordinator,
                )
            }))
            .unwrap_or_else(|_| {
                let mut receipt = crate::tool_orchestrator::tool_receipt_from_runtime(
                    agent_run_failed_tool_receipt(
                        &fallback_call,
                        "provider tool-call 执行线程异常退出",
                    ),
                );
                receipt.tool_call_id = fallback_id;
                receipt.part_id = fallback_part;
                timed_tool_receipt(receipt, Instant::now())
            });
            let _ = task_events.send(ParallelToolProgress::Finished { index, receipt });
        });
        if let Err(reason) = submitted {
            // 池不可用不能被记成"用户取消"：直接产出带原因的失败回执。
            let message = format!("工具工作池不可用：{reason}");
            let mut receipt = crate::tool_orchestrator::tool_receipt_from_runtime(
                agent_run_failed_tool_receipt(&execution_request.tool_call, message.as_str()),
            );
            attach_tool_call_id(&mut receipt, execution_request);
            let mut timed = timed_tool_receipt(receipt, Instant::now());
            timed.duration_ms = Some(0);
            let _ = event_tx.send(ParallelToolProgress::Finished {
                index,
                receipt: timed,
            });
        }
        started += 1;
    }
    drop(event_tx);

    let mut slots: Vec<Option<TimedToolReceipt>> = (0..total).map(|_| None).collect();
    let mut receipts: Vec<Option<RunToolReceipt>> = (0..total).map(|_| None).collect();
    let mut committed = 0usize;
    let mut finished = 0usize;
    while finished < started {
        match event_rx.recv() {
            Ok(ParallelToolProgress::Started { index }) => {
                if let Some(sink) = progress {
                    sink.tool_execution_started(&execution_requests[index]);
                }
            }
            Ok(ParallelToolProgress::RuntimeEvent(event)) => {
                if let Some(sink) = progress {
                    sink.tool_runtime_event(event);
                }
            }
            Ok(ParallelToolProgress::Finished { index, receipt }) => {
                finished += 1;
                // 完成顺序不等于提交顺序：先入槽位，再由连续游标按模型序提交。
                slots[index] = Some(receipt);
                commit_ready_receipts(
                    &mut slots,
                    &mut receipts,
                    &mut committed,
                    execution_requests,
                    progress,
                    &mut |_| {},
                );
            }
            Err(_) => break,
        }
    }
    commit_ready_receipts(
        &mut slots,
        &mut receipts,
        &mut committed,
        execution_requests,
        progress,
        &mut |_| {},
    );
    // 游标之后可能还有已到达但未提交的回执，先按序落位，再给真正未启动的调用补合成回执。
    fill_unstarted_receipts(
        &mut slots,
        &mut receipts,
        committed,
        execution_requests,
        progress,
    );
    receipts
        .into_iter()
        .enumerate()
        .map(|(index, receipt)| {
            receipt.unwrap_or_else(|| {
                let execution_request = &execution_requests[index];
                let mut receipt = crate::tool_orchestrator::tool_receipt_from_runtime(
                    agent_run_failed_tool_receipt(
                        &execution_request.tool_call,
                        "provider tool-call 执行未返回回执",
                    ),
                );
                attach_tool_call_id(&mut receipt, execution_request);
                receipt
            })
        })
        .collect()
}

/// 整批已取消时，为每个 call 生成配对的合成回执（`S-6`、`I-7`）。
fn cancelled_receipts(
    execution_requests: &[ToolExecutionRequest],
    progress: Option<&RunProgressSink<'_>>,
) -> Vec<RunToolReceipt> {
    execution_requests
        .iter()
        .map(|execution_request| {
            let mut receipt =
                crate::tool_orchestrator::tool_receipt_from_runtime(agent_run_failed_tool_receipt(
                    &execution_request.tool_call,
                    "任务已由用户中断，未启动该工具调用",
                ));
            attach_tool_call_id(&mut receipt, execution_request);
            if let Some(sink) = progress {
                sink.tool_execution_completed(execution_request, &receipt, None);
            }
            receipt
        })
        .collect()
}

/// 收口剩余槽位（`S-6`）：已到达的真实回执按序落位，真正未启动的才补合成回执。
fn fill_unstarted_receipts(
    slots: &mut [Option<TimedToolReceipt>],
    receipts: &mut [Option<RunToolReceipt>],
    committed: usize,
    execution_requests: &[ToolExecutionRequest],
    progress: Option<&RunProgressSink<'_>>,
) {
    for index in committed..execution_requests.len() {
        let execution_request = &execution_requests[index];
        if let Some(timed) = slots[index].take() {
            if let Some(sink) = progress {
                sink.tool_execution_completed(execution_request, &timed.receipt, timed.duration_ms);
            }
            receipts[index] = Some(timed.receipt);
            continue;
        }
        let mut receipt =
            crate::tool_orchestrator::tool_receipt_from_runtime(agent_run_failed_tool_receipt(
                &execution_request.tool_call,
                "任务已由用户中断，未启动该工具调用",
            ));
        attach_tool_call_id(&mut receipt, execution_request);
        if let Some(sink) = progress {
            sink.tool_execution_completed(execution_request, &receipt, None);
        }
        receipts[index] = Some(receipt);
    }
}

/// 连续提交游标：只有游标槽位就绪才提交并前进。
fn commit_ready_receipts(
    slots: &mut [Option<TimedToolReceipt>],
    receipts: &mut [Option<RunToolReceipt>],
    committed: &mut usize,
    execution_requests: &[ToolExecutionRequest],
    progress: Option<&RunProgressSink<'_>>,
    on_commit: &mut dyn FnMut(usize),
) {
    while *committed < slots.len() {
        let Some(timed) = slots[*committed].take() else {
            break;
        };
        if let Some(sink) = progress {
            sink.tool_execution_completed(
                &execution_requests[*committed],
                &timed.receipt,
                timed.duration_ms,
            );
        }
        receipts[*committed] = Some(timed.receipt);
        on_commit(*committed);
        *committed += 1;
    }
}

/// 测试缝：用可注入延迟的假执行器跑真实的池调度与提交游标。
///
/// 只用于门禁证据，生产路径仍走 `execute_pooled_tool`。
#[cfg(test)]
pub(super) fn execute_parallel_for_tests(
    execution_requests: &[ToolExecutionRequest],
    latency_ms: &[u64],
    start_limit: Option<usize>,
    commit_log: &mut Vec<usize>,
) -> Vec<RunToolReceipt> {
    let total = execution_requests.len();
    let pool = session_tool_pool("parallel-seam-test", CapacitySnapshot::preset());
    let (event_tx, event_rx) = mpsc::channel();
    let mut started = 0usize;
    for (index, execution_request) in execution_requests.iter().enumerate() {
        // 取消后不再启动新调用，未启动的调用稍后补合成回执。
        if start_limit.is_some_and(|limit| started >= limit) {
            break;
        }
        started += 1;
        let event_tx = event_tx.clone();
        let delay = latency_ms.get(index).copied().unwrap_or(0);
        let call = execution_request.tool_call.clone();
        let part_id = execution_request.part_id.clone();
        let call_id = execution_request.tool_call_id.0;
        pool.submit(move || {
            std::thread::sleep(std::time::Duration::from_millis(delay));
            let mut receipt = crate::tool_orchestrator::tool_receipt_from_runtime(
                agent_run_failed_tool_receipt(&call, "seam"),
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
fn is_run_cancelled(context: &ToolOrchestratorContext) -> bool {
    context
        .run_cancel_token
        .as_ref()
        .is_some_and(super::RunCancelToken::is_cancelled)
}

fn execute_pooled_tool(
    context: &ToolOrchestratorContext,
    execution_request: &ToolExecutionRequest,
    index: usize,
    total: usize,
    event_tx: mpsc::Sender<ParallelToolProgress>,
    write_coordinator: &WorkspaceWriteCoordinator,
) -> TimedToolReceipt {
    // `S-5`：每次启动前按最新参数重新分类，切段用的是批次快照，取锁用的是启动时事实。
    let scope = tool_access_scope(&context.cwd, execution_request);
    let tool_call = &execution_request.tool_call;
    let started_at = Instant::now();
    if is_run_cancelled(context) {
        let mut receipt = crate::tool_orchestrator::tool_receipt_from_runtime(
            agent_run_failed_tool_receipt(tool_call, "任务已由用户中断，未启动该工具调用"),
        );
        attach_tool_call_id(&mut receipt, execution_request);
        return timed_tool_receipt(receipt, started_at);
    }
    if context.is_cancelled() {
        let mut receipt = cancelled_tool_receipt(tool_call);
        attach_tool_call_id(&mut receipt, execution_request);
        receipt.duration_ms = Some(elapsed_ms(started_at));
        return TimedToolReceipt {
            duration_ms: receipt.duration_ms,
            receipt,
        };
    }
    let _ = event_tx.send(ParallelToolProgress::RuntimeEvent(
        parallel_tool_batch_progress(execution_request, index, total, started_at),
    ));
    let mut observer = move |event: ToolRuntimeEvent| {
        let _ = event_tx.send(ParallelToolProgress::RuntimeEvent(event));
    };
    let mut receipt = write_coordinator.execute(&scope, || {
        if is_run_cancelled(context) || context.is_cancelled() {
            return cancelled_tool_receipt(tool_call);
        }
        execute_tool_call_with_cache(context, execution_request, Some(&mut observer))
    });
    attach_tool_call_id(&mut receipt, execution_request);
    // 工具已返回时保留真实回执：它可能已经写盘，替换成"已取消"会让模型重复执行。
    receipt.duration_ms = Some(elapsed_ms(started_at));
    timed_tool_receipt(receipt, started_at)
}

fn timed_tool_receipt(mut receipt: RunToolReceipt, started_at: Instant) -> TimedToolReceipt {
    let duration_ms = Some(elapsed_ms(started_at));
    if receipt.duration_ms.is_none() {
        receipt.duration_ms = duration_ms;
    }
    TimedToolReceipt {
        receipt,
        duration_ms,
    }
}

fn elapsed_ms(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn parallel_tool_batch_progress(
    execution_request: &ToolExecutionRequest,
    index: usize,
    total: usize,
    started_at: Instant,
) -> ToolRuntimeEvent {
    ToolRuntimeEvent::Progress(ToolProgressInfo {
        tool_call_id: execution_request.tool_call_id,
        part_id: execution_request.part_id.clone(),
        provider_call_id: execution_request.tool_call.id.clone(),
        tool_name: execution_request.tool_call.dispatch_name().to_owned(),
        kind: execution_request.kind,
        phase: "batch_running".to_owned(),
        items_completed: Some(u64::try_from(index.saturating_add(1)).unwrap_or(u64::MAX)),
        items_total: Some(u64::try_from(total).unwrap_or(u64::MAX)),
        current_item: Some(execution_request.tool_call.dispatch_name().to_owned()),
        elapsed_ms: Some(elapsed_ms(started_at)),
        message: Some("并行工具批次执行中".to_owned()),
    })
}

fn cancelled_tool_receipt(tool_call: &ProviderToolCall) -> RunToolReceipt {
    crate::tool_orchestrator::tool_receipt_from_runtime(agent_run_failed_tool_receipt(
        tool_call,
        "任务已由用户中断",
    ))
}

enum ParallelToolProgress {
    Started {
        index: usize,
    },
    RuntimeEvent(ToolRuntimeEvent),
    Finished {
        index: usize,
        receipt: TimedToolReceipt,
    },
}

/// 按读写交叉把批次切成覆盖全部请求的连续子段。
///
/// 独占请求独占一段；写集相交或读写同路径的两个请求必须落在不同段，
/// 段与段之间严格保持模型给出的调用顺序，因此切分不会翻转因果次序。
pub(super) fn tool_batch_ranges(scopes: &[ToolAccessScope]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut segment_start = 0usize;
    for (index, scope) in scopes.iter().enumerate() {
        // 独占与整工作区写都必须自成一段，前后都是屏障（`S-2`）。
        if scope.is_isolated() {
            if segment_start < index {
                ranges.push(segment_start..index);
            }
            ranges.push(index..index + 1);
            segment_start = index + 1;
            continue;
        }
    }
    if segment_start < scopes.len() {
        ranges.push(segment_start..scopes.len());
    }
    ranges
}

#[cfg(test)]
fn unique_test_thread_session_id(prefix: &str) -> String {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{prefix}-{}-{id}", std::process::id())
}

#[cfg(test)]
#[path = "run_tool_loop_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "run_tool_loop_subagent_tests.rs"]
mod subagent_tests;

#[cfg(test)]
#[path = "run_tool_loop_subagent_restore_tests.rs"]
mod subagent_restore_tests;

#[cfg(test)]
#[path = "run_tool_loop_subagent_path_tests.rs"]
mod subagent_path_tests;

#[cfg(test)]
#[path = "run_tool_loop_subagent_graph_tests.rs"]
mod subagent_graph_tests;

#[cfg(test)]
#[path = "run_tool_loop_subagent_job_tests.rs"]
mod subagent_job_tests;
