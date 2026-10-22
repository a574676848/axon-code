use std::{
    collections::HashSet,
    ops::Range,
    sync::{Arc, mpsc},
    thread,
    time::Instant,
};

use axon_agent_runtime::{ProviderToolCall, SPAWN_AGENT_TOOL_NAME, agent_run_failed_tool_receipt};
use axon_domain::{CellId, SessionId, ToolConcurrencyPolicy};

use super::{
    RunRequest,
    run_progress::RunProgressSink,
    run_tool_result_cache::execute_tool_call_with_cache,
    run_tool_write_coordination::{
        WorkspaceWriteCoordinator, tool_write_scope, workspace_write_coordinator,
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
    if run_cancelled(request) {
        return Vec::new();
    }
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
    if run_cancelled(request) {
        return Vec::new();
    }
    // 批次按并发策略切成连续子段：段内可并发，段间严格顺序。
    // 独占请求自成一段，因此仍不会与同批其他工具重叠。
    let mut receipts = Vec::with_capacity(execution_requests.len());
    for range in parallel_tool_batch_ranges(&execution_requests) {
        let batch = &execution_requests[range];
        if tool_calls_can_run_in_parallel(batch) {
            receipts.extend(execute_parallel_tool_calls(
                &context, request, batch, progress,
            ));
            continue;
        }
        for execution_request in batch {
            receipts.push(execute_supported_tool_call(
                &context,
                request,
                execution_request,
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
    let write_scope = tool_write_scope(&context.cwd, execution_request);
    let mut receipt = write_coordinator.execute(&write_scope, || {
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
    if context.is_cancelled() {
        let mut receipt = cancelled_tool_receipt(tool_call);
        attach_tool_call_id(&mut receipt, execution_request);
        receipt.duration_ms = Some(elapsed_ms(started_at));
        emit_tool_completed(
            progress,
            execution_request,
            &receipt,
            Some(elapsed_ms(started_at)),
        );
        return receipt;
    }
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
            SPAWN_AGENT_TOOL_NAME
                | "builtin.spawn_agent"
                | axon_agent_runtime::DELEGATE_TO_FAST_TOOL_NAME
                | "builtin.delegate_to_fast"
        )
}

fn execute_parallel_tool_calls(
    context: &ToolOrchestratorContext,
    request: &RunRequest,
    execution_requests: &[ToolExecutionRequest],
    progress: Option<&RunProgressSink<'_>>,
) -> Vec<RunToolReceipt> {
    if let Some(sink) = progress {
        for execution_request in execution_requests {
            sink.tool_execution_started(execution_request);
        }
    }
    let write_coordinator = workspace_write_coordinator(&context.cwd);
    thread::scope(|scope| {
        let (event_tx, event_rx) = mpsc::channel();
        let handles = execution_requests
            .iter()
            .enumerate()
            .map(|(index, execution_request)| {
                let event_tx = event_tx.clone();
                let write_coordinator = Arc::clone(&write_coordinator);
                scope.spawn(move || {
                    let receipt = execute_parallel_tool_worker(
                        context,
                        request,
                        execution_request,
                        index,
                        execution_requests.len(),
                        event_tx.clone(),
                        &write_coordinator,
                    );
                    let _ = event_tx.send(ParallelToolProgress::Finished { index, receipt });
                })
            })
            .collect::<Vec<_>>();
        drop(event_tx);

        let mut receipts = vec![None; execution_requests.len()];
        for event in event_rx {
            match event {
                ParallelToolProgress::RuntimeEvent(event) => {
                    if let Some(sink) = progress {
                        sink.tool_runtime_event(event);
                    }
                }
                ParallelToolProgress::Finished { index, receipt } => {
                    if let Some(sink) = progress {
                        let duration_ms = receipt.duration_ms;
                        sink.tool_execution_completed(
                            &execution_requests[index],
                            &receipt.receipt,
                            duration_ms,
                        );
                    }
                    receipts[index] = Some(receipt.receipt);
                }
            }
        }

        for (index, (execution_request, handle)) in
            execution_requests.iter().zip(handles).enumerate()
        {
            if handle.join().is_err() && receipts[index].is_none() {
                let mut receipt = crate::tool_orchestrator::tool_receipt_from_runtime(
                    agent_run_failed_tool_receipt(
                        &execution_request.tool_call,
                        "provider tool-call 执行线程异常退出",
                    ),
                );
                attach_tool_call_id(&mut receipt, execution_request);
                if let Some(sink) = progress {
                    sink.tool_execution_completed(execution_request, &receipt, None);
                }
                receipts[index] = Some(receipt);
            }
        }

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
                    if let Some(sink) = progress {
                        sink.tool_execution_completed(execution_request, &receipt, None);
                    }
                    receipt
                })
            })
            .collect()
    })
}

fn execute_parallel_tool_worker(
    context: &ToolOrchestratorContext,
    request: &RunRequest,
    execution_request: &ToolExecutionRequest,
    index: usize,
    total: usize,
    event_tx: mpsc::Sender<ParallelToolProgress>,
    write_coordinator: &WorkspaceWriteCoordinator,
) -> TimedToolReceipt {
    let tool_call = &execution_request.tool_call;
    let started_at = Instant::now();
    if run_cancelled(request) {
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
    let write_scope = tool_write_scope(&context.cwd, execution_request);
    let mut receipt = write_coordinator.execute(&write_scope, || {
        if run_cancelled(request) || context.is_cancelled() {
            return cancelled_tool_receipt(tool_call);
        }
        execute_tool_call_with_cache(context, execution_request, Some(&mut observer))
    });
    attach_tool_call_id(&mut receipt, execution_request);
    if context.is_cancelled() {
        let mut receipt = cancelled_tool_receipt(tool_call);
        attach_tool_call_id(&mut receipt, execution_request);
        return timed_tool_receipt(receipt, started_at);
    }
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
    RuntimeEvent(ToolRuntimeEvent),
    Finished {
        index: usize,
        receipt: TimedToolReceipt,
    },
}

/// 按并发策略把批次切成覆盖全部请求的连续子段。
///
/// `Exclusive` 请求独占一段，任何并发都不会把同批其他工具带进它的执行窗口；
/// 其余请求合并成一段以便并发。段与段之间严格保持模型给出的调用顺序，
/// 因此切分不会翻转工具之间的因果次序。
fn parallel_tool_batch_ranges(execution_requests: &[ToolExecutionRequest]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut segment_start = 0usize;
    for (index, execution_request) in execution_requests.iter().enumerate() {
        if execution_request.concurrency_policy != ToolConcurrencyPolicy::Exclusive {
            continue;
        }
        if segment_start < index {
            ranges.push(segment_start..index);
        }
        ranges.push(index..index + 1);
        segment_start = index + 1;
    }
    if segment_start < execution_requests.len() {
        ranges.push(segment_start..execution_requests.len());
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
