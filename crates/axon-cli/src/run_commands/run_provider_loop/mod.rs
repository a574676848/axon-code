use axon_agent_runtime::{
    AgentModelHistorySnapshot, AgentRunCompletedInput, AgentRunReasoningMessage, AgentTurnEvent,
    AgentTurnEventKind, AgentTurnRuntime, AgentTurnSteerSource, CompressionPolicy,
    ProviderStreamProtocol, ProviderStreamingTransportError, ProviderStreamingTransportInput,
    ProviderStreamingTransportOutcome, ProviderStreamingTransportReport,
    ProviderStreamingTransportRequest, WindowRolloverPolicyRuntime, WindowRolloverRuntime,
    agent_run_execution_error_from_axon_error, agent_run_interrupted_error,
    agent_run_provider_reasoning_id, agent_run_reasoning_part_id,
    agent_run_response_from_turn_report, agent_run_tool_calls_message,
    estimated_history_tokens_for_provider_with_capabilities,
};
use axon_config::{ModelConfig, ProviderConfig};
use axon_protocol::wire::{
    WIRE_LIFECYCLE_KIND_PROVIDER_RETRY, WireProviderStatus, WireProviderStatusPhase,
    WireProviderTransport,
};
use axon_provider_transport::{
    AgentRunProviderRequestInput, AgentRunProviderTransportSession, agent_run_send_provider_request,
};
use axon_session_runtime::TurnPerformanceStage;
use std::{
    cell::{Cell, RefCell},
    time::Duration,
};

use super::RunRequest;
use super::run_channel_affinity::record_channel_connection_observation;
use super::run_execution::{RunExecutionError, RunResponse, RunToolReceipt};
use super::run_progress::{RunLifecycleStatusView, RunProgressSink, RunProgressStreamNormalizer};
use super::run_steer::RunSteerInbox;
use super::run_tool_loop::execute_supported_tool_calls_for_turn;
use super::run_tool_result_wire::tool_decision;

#[path = "inline_compaction.rs"]
mod inline_compaction;
mod quoted_message_input;
#[cfg(test)]
mod quoted_message_input_tests;
mod settlement;
#[path = "steer_progress.rs"]
mod steer_progress;
#[cfg(test)]
mod tests;
#[path = "tool_lifecycle.rs"]
mod tool_lifecycle;
mod tool_output_archive;

#[path = "visible_output.rs"]
mod visible_output;

use settlement::*;

use inline_compaction::InlineWindowRollover;
use quoted_message_input::provider_streaming_transport_input;
use steer_progress::{BackgroundJobGuardSteerSource, ProgressSteerSource};
use tool_lifecycle::tool_lifecycle_turn_events;
use tool_output_archive::archive_oversized_tool_receipts;
use visible_output::observe_visible_output;

const LIFECYCLE_TITLE_RUNNING: &str = "运行";
const LIFECYCLE_SUMMARY_PROVIDER_RETRY: &str = "连接失败，正在重试";
const LIFECYCLE_PHASE_STREAMING: &str = "streaming";
const TOOL_PHASE: &str = "tool";

#[derive(Clone)]
struct ProviderTransportPlan {
    provider_name: String,
    model_id: String,
    logical_session: String,
    endpoint: String,
    wire_transport: axon_agent_runtime::ProviderStreamingWireTransport,
    websocket_url: Option<String>,
    enable_request_compression: bool,
    stream_protocol: ProviderStreamProtocol,
    timeout: Duration,
}

struct PartialToolCallState<'a> {
    tool_receipts: &'a RefCell<Vec<RunToolReceipt>>,
    tool_calls: &'a RefCell<Vec<axon_agent_runtime::ProviderToolCall>>,
    reasoning: &'a RefCell<Vec<AgentRunReasoningMessage>>,
    turn_events: &'a RefCell<Vec<AgentTurnEvent>>,
    provider_name: &'a str,
    model_id: &'a str,
}

/// 跨渠道续跑现场。
///
/// 渠道在还没收到任何上游响应时失败，本次 provider turn 没有产生任何输出；把该 turn
/// 的起始历史与已落地事实存下来，下一渠道就能从同一位置续跑，不会重放已经执行过的
/// 工具副作用。
#[derive(Clone)]
pub(super) struct ChannelResumeState {
    pub(super) messages: axon_agent_runtime::AgentModelHistorySnapshot,
    pub(super) tool_receipts: Vec<RunToolReceipt>,
    pub(super) tool_calls: Vec<axon_agent_runtime::ProviderToolCall>,
    pub(super) reasoning: Vec<AgentRunReasoningMessage>,
    pub(super) turn_events: Vec<AgentTurnEvent>,
}

impl ProviderTransportPlan {
    fn from_input(
        input: &axon_agent_runtime::ProviderStreamingTransportInput,
        logical_session: &str,
        provider: &ProviderConfig,
        model: &ModelConfig,
        timeout: Duration,
    ) -> Self {
        Self {
            provider_name: provider.name.clone(),
            model_id: model.id.clone(),
            logical_session: logical_session.to_owned(),
            endpoint: input.base_url.clone(),
            wire_transport: input.wire_transport,
            websocket_url: input.websocket_url.clone(),
            enable_request_compression: input.enable_request_compression,
            stream_protocol: input.protocol,
            timeout,
        }
    }
}

/// 单渠道执行单元：在指定 (model, provider) 上跑完整的 provider tool loop。
///
/// 从故障转移循环中按渠道调用；`channel_provider`/`channel_model` 即本次尝试的渠道，
/// 可能不同于 `request` 的首选渠道。
///
/// 边界约束：runtime 负责 provider/turn/tool decision 的可复用合同；本函数只保留 CLI
/// adapter 接线，包括本机 LSP 预热、插件 registry 加载、本机工具执行和 verify 采集。
pub(super) fn run_provider_single_channel(
    request: &RunRequest,
    channel_model: &ModelConfig,
    channel_provider: &ProviderConfig,
    progress: Option<&RunProgressSink<'_>>,
    turn_inbox: &RunSteerInbox,
    resume: Option<ChannelResumeState>,
    resume_checkpoint: &RefCell<Option<ChannelResumeState>>,
) -> Result<RunResponse, Box<RunExecutionError>> {
    axon_logger::debug(&format!(
        "run_provider_single_channel 开始: provider={}, model={}, wire_transport={}",
        channel_provider.name, channel_model.id, channel_provider.wire_transport
    ));
    if run_cancelled(request) {
        return Err(interrupted_error(&[]));
    }
    let _token_budget_snapshot_scope =
        crate::tool_orchestrator::ProviderTokenBudgetSnapshotScope::new(
            &request.session_id,
            &request.invocation_id,
        );
    // 只有渠道链里还有下一个渠道时才值得留续跑现场，否则记下来也没人消费。
    let has_next_channel = request
        .channel_chain
        .iter()
        .position(|(model, provider)| {
            model.id == channel_model.id && provider.name == channel_provider.name
        })
        .is_some_and(|index| index + 1 < request.channel_chain.len());
    let connect_failure_checkpointed = Cell::new(false);
    let mut input = provider_streaming_transport_input(request, channel_model, channel_provider)?;
    if let Some(state) = resume.as_ref() {
        // 上一渠道连不上，本 turn 没有任何上游输出：从该 turn 的起始历史续跑。
        input.messages = state.messages.clone();
        input.prompt_cache_plan = axon_agent_runtime::provider_streaming_prompt_cache_plan(&input)
            .map_err(|error| {
                Box::new(agent_run_execution_error_from_axon_error::<RunToolReceipt>(
                    &error,
                    &[],
                ))
            })?;
    }
    // 渠道 failover 会在同一个 run 内重跑整个 turn loop：本次尝试必须从上一尝试之后的
    // turn 序号起步，assistant/reasoning part 身份才不会跨尝试碰撞。
    input.turn_id = axon_domain::TurnId(begin_provider_turn_epoch(progress));
    if let Some(performance) = &request.performance {
        performance.record(TurnPerformanceStage::ProviderInputReady);
    }
    let transport_plan = ProviderTransportPlan::from_input(
        &input,
        &request.session_id,
        channel_provider,
        channel_model,
        request.timeout,
    );
    let transport_session = AgentRunProviderTransportSession::new(
        &transport_plan.provider_name,
        &transport_plan.logical_session,
        &transport_plan.endpoint,
        transport_plan.wire_transport,
    );
    let tool_receipts_accumulator = RefCell::new(
        resume
            .as_ref()
            .map(|state| state.tool_receipts.clone())
            .unwrap_or_default(),
    );
    let tool_calls_accumulator = RefCell::new(
        resume
            .as_ref()
            .map(|state| state.tool_calls.clone())
            .unwrap_or_default(),
    );
    let reasoning_accumulator = RefCell::new(
        resume
            .as_ref()
            .map(|state| state.reasoning.clone())
            .unwrap_or_default(),
    );
    let turn_events_accumulator = RefCell::new(
        resume
            .as_ref()
            .map(|state| state.turn_events.clone())
            .unwrap_or_default(),
    );
    let visible_output_emitted = Cell::new(false);
    let partial_state = PartialToolCallState {
        tool_receipts: &tool_receipts_accumulator,
        tool_calls: &tool_calls_accumulator,
        reasoning: &reasoning_accumulator,
        turn_events: &turn_events_accumulator,
        provider_name: &transport_plan.provider_name,
        model_id: &transport_plan.model_id,
    };

    let compression_policy = CompressionPolicy::for_context_window(
        channel_model.context_window,
        request.auto_compaction_threshold,
    )
    .map_err(|error| {
        Box::new(agent_run_execution_error_from_axon_error::<RunToolReceipt>(
            &error,
            &[],
        ))
    })?;
    let rollover_limit_tokens =
        compression_policy.map_or(channel_model.context_window, |policy| policy.trigger_tokens);
    let mut rollover = InlineWindowRollover::new(request, progress);
    let compression = compression_policy.map(|policy| {
        WindowRolloverRuntime::new(WindowRolloverPolicyRuntime::new(policy), &mut rollover)
    });
    let steer_source = BackgroundJobGuardSteerSource::new(
        ProgressSteerSource::new(turn_inbox, progress),
        request.session_id.clone(),
    );
    if let Some(performance) = &request.performance {
        performance.record(TurnPerformanceStage::ProviderRuntimeReady);
    }

    // 阶段计时分级输出：每轮 debug（默认级别不落盘）；单阶段超过阈值升 warning；
    // run 收尾汇总一条（正常 info，慢 run 升 warning 作为归因锚点）。
    // 有界输出，避免无界日志膨胀浪费空间。
    const PROVIDER_EXCHANGE_WARN_MS: u128 = 60_000;
    const TOOL_EXECUTION_WARN_MS: u128 = 5_000;
    const TURN_ORCHESTRATION_WARN_MS: u128 = 120_000;
    let turn_orchestration_started = std::time::Instant::now();
    let provider_exchange_total_ms = Cell::new(0u64);
    let provider_exchange_rounds = Cell::new(0u32);
    let tool_execution_total_ms = Cell::new(0u64);
    let tool_execution_rounds = Cell::new(0u32);

    let report = AgentTurnRuntime
        .run(
            &input,
            |transport_request| {
                let provider_exchange_started = std::time::Instant::now();
                let outcome = send_provider_transport(
                    request,
                    &transport_plan,
                    &transport_session,
                    progress,
                    transport_request,
                );
                // 连接阶段失败才留现场：此时本 turn 一个 Provider 事件都没收到，
                // 换渠道从同一历史续跑不会重放已经落地的工具副作用。
                resume_checkpoint.replace(None);
                connect_failure_checkpointed.set(false);
                if has_next_channel
                    && let Some(state) =
                        connect_failure_resume_state(&outcome, transport_request, &partial_state)
                {
                    resume_checkpoint.replace(Some(state));
                    connect_failure_checkpointed.set(true);
                }
                let provider_exchange_ms = u64::try_from(
                    provider_exchange_started.elapsed().as_millis(),
                )
                .unwrap_or(u64::MAX);
                provider_exchange_total_ms
                    .set(provider_exchange_total_ms.get() + provider_exchange_ms);
                provider_exchange_rounds.set(provider_exchange_rounds.get() + 1);
                if provider_exchange_ms as u128 >= PROVIDER_EXCHANGE_WARN_MS {
                    axon_logger::warning(&format!(
                        "provider 交换耗时异常: session={}, turn={}, ms={provider_exchange_ms}",
                        request.session_id,
                        transport_request.turn_id.0
                    ));
                } else {
                    axon_logger::debug(&format!(
                        "turn 阶段计时: session={}, phase=provider_exchange, turn={}, ms={provider_exchange_ms}",
                        request.session_id,
                        transport_request.turn_id.0
                    ));
                }
                if matches!(
                    &outcome,
                    ProviderStreamingTransportOutcome::VisibleOutput(_)
                ) {
                    visible_output_emitted.set(true);
                }
                outcome
            },
            |transport_report, provider_history, turn_id| {
                let tool_execution_started = std::time::Instant::now();
                let decision = build_turn_tool_decision(
                    request,
                    progress,
                    transport_report,
                    provider_history,
                    &input,
                    turn_id.0,
                    rollover_limit_tokens,
                    &partial_state,
                );
                let tool_execution_ms =
                    u64::try_from(tool_execution_started.elapsed().as_millis()).unwrap_or(u64::MAX);
                tool_execution_total_ms.set(tool_execution_total_ms.get() + tool_execution_ms);
                tool_execution_rounds.set(tool_execution_rounds.get() + 1);
                if tool_execution_ms as u128 >= TOOL_EXECUTION_WARN_MS {
                    axon_logger::warning(&format!(
                        "工具执行耗时异常: session={}, turn={}, ms={tool_execution_ms}",
                        request.session_id,
                        turn_id.0
                    ));
                } else {
                    axon_logger::debug(&format!(
                        "turn 阶段计时: session={}, phase=tool_execution, turn={}, ms={tool_execution_ms}",
                        request.session_id,
                        turn_id.0
                    ));
                }
                decision
            },
            Some(&steer_source as &dyn AgentTurnSteerSource),
            compression,
        )
        .map_err(|error| {
            if run_cancelled(request) {
                return interrupted_error(tool_receipts_accumulator.borrow().as_slice());
            }
            let mut execution_error = agent_run_execution_error_from_axon_error(
                &error,
                tool_receipts_accumulator.borrow().as_slice(),
            );
            execution_error.provider_protocol =
                Some(transport_plan.stream_protocol.as_code().to_owned());
            execution_error.visible_output_emitted = visible_output_emitted.get();
            // 已写文件的工具不可重放：把它落成显式事实，跨渠道续跑判定才不需要猜。
            execution_error.committed_side_effects = tool_receipts_accumulator
                .borrow()
                .iter()
                .any(|receipt| receipt.filesystem_write_performed);
            execution_error.resumable_connect_failure = connect_failure_checkpointed.get();
            Box::new(execution_error)
        })?;
    let turn_orchestration_total_ms =
        u64::try_from(turn_orchestration_started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let turn_phase_summary = format!(
        "turn 阶段汇总: session={}, total_ms={turn_orchestration_total_ms}, provider_rounds={}, provider_ms={}, tool_rounds={}, tool_ms={}",
        request.session_id,
        provider_exchange_rounds.get(),
        provider_exchange_total_ms.get(),
        tool_execution_rounds.get(),
        tool_execution_total_ms.get()
    );
    if turn_orchestration_total_ms as u128 >= TURN_ORCHESTRATION_WARN_MS {
        // 慢 run 在 Warning+ 级部署里也可见：总耗时减两阶段耗时即 harness 间隔。
        axon_logger::warning(&format!("慢 run 归因: {turn_phase_summary}"));
    } else {
        axon_logger::info(&turn_phase_summary);
    }
    emit_provider_reasoning_reports(progress, &report.turn_reports);
    let mut response = response_from_turn_report(report, channel_provider, channel_model);
    if let Some(state) = resume.as_ref() {
        prepend_resume_state(&mut response, state);
    }
    Ok(response)
}

fn send_provider_transport(
    request: &RunRequest,
    plan: &ProviderTransportPlan,
    transport_session: &AgentRunProviderTransportSession,
    progress: Option<&RunProgressSink<'_>>,
    transport_request: &ProviderStreamingTransportRequest,
) -> ProviderStreamingTransportOutcome {
    if run_cancelled(request) {
        return interrupted_transport_outcome();
    }
    if let Some(sink) = progress {
        sink.observe_provider_turn(transport_request.turn_id.0);
    }
    emit_streaming_started(
        plan,
        transport_session.effective_transport(),
        progress,
        transport_request.attempt_index,
    );
    if let Some(sink) = progress {
        // 构成随本次尝试的每一条用量事件投出：收尾补发与实时帧同形，重复帧才可识别。
        sink.begin_attempt_composition(transport_request.context_composition);
    }
    if let Some(performance) = &request.performance {
        performance.record(TurnPerformanceStage::StreamingStarted);
    }
    let response = send_provider_transport_request(
        request,
        plan,
        transport_session,
        transport_request,
        progress,
    );
    emit_streaming_completed(
        plan,
        progress,
        transport_request.attempt_index,
        &response,
        transport_request.context_composition,
    );
    if run_cancelled(request) {
        return interrupted_transport_outcome();
    }
    response
}

#[path = "provider_transport_attempt.rs"]
mod provider_transport_attempt;
use provider_transport_attempt::*;

fn run_cancelled(request: &RunRequest) -> bool {
    request
        .cancel_token
        .as_ref()
        .is_some_and(super::RunCancelToken::is_cancelled)
}

fn build_turn_tool_decision(
    request: &RunRequest,
    progress: Option<&RunProgressSink<'_>>,
    transport_report: &ProviderStreamingTransportReport,
    provider_history: &AgentModelHistorySnapshot,
    transport_input: &ProviderStreamingTransportInput,
    turn_id: u64,
    rollover_limit_tokens: u64,
    partial_state: &PartialToolCallState<'_>,
) -> axon_domain::AxonResult<axon_agent_runtime::AgentTurnToolDecision<RunToolReceipt>> {
    if run_cancelled(request) {
        return interrupted_axon_error();
    }
    partial_state
        .tool_calls
        .borrow_mut()
        .extend(transport_report.runner_report.tool_calls.clone());
    if let Some(content) = transport_report
        .runner_report
        .reasoning_content
        .as_deref()
        .map(str::trim)
        .filter(|content| !content.is_empty())
    {
        partial_state
            .reasoning
            .borrow_mut()
            .push(AgentRunReasoningMessage {
                part_id: agent_run_reasoning_part_id(turn_id),
                reasoning_id: agent_run_provider_reasoning_id(turn_id),
                content: content.to_owned(),
            });
    }
    let pending_events =
        partial_tool_call_events(turn_id, &transport_report.runner_report.tool_calls);
    partial_state
        .turn_events
        .borrow_mut()
        .extend(pending_events);
    persist_partial_tool_call_record(request, partial_state, turn_id);
    let mut locally_observed_history = provider_history.as_slice().to_vec();
    locally_observed_history.push(agent_run_tool_calls_message(
        &transport_report.runner_report.tool_calls,
        transport_report.runner_report.reasoning_content.as_deref(),
    ));
    let local_estimated_context_tokens = estimated_history_tokens_for_provider_with_capabilities(
        &locally_observed_history,
        provider_wire_format(transport_input.protocol),
        &transport_input.model_tools,
        transport_input.supports_vision,
    );
    crate::tool_orchestrator::record_provider_token_budget_snapshot(
        &request.session_id,
        &request.invocation_id,
        rollover_limit_tokens,
        transport_report
            .runner_report
            .token_usage
            .map(|usage| usage.current_context_tokens)
            // 未知（0）不写入快照：累计量不是当前上下文，不能拿来顶替。
            .filter(|tokens| *tokens > 0),
        local_estimated_context_tokens,
    );
    let mut turn_tool_receipts = execute_supported_tool_calls_for_turn(
        request,
        &transport_report.runner_report.tool_calls,
        turn_id,
        progress,
    );
    archive_oversized_tool_receipts(request, &mut turn_tool_receipts);
    if run_cancelled(request) {
        persist_settled_tool_receipts(request, partial_state, turn_id, &turn_tool_receipts);
        return interrupted_axon_error();
    }

    persist_settled_tool_receipts(request, partial_state, turn_id, &turn_tool_receipts);

    tool_decision(
        &transport_report.runner_report.tool_calls,
        turn_tool_receipts,
        transport_report.runner_report.reasoning_content.as_deref(),
    )
    .map_err(|_| axon_domain::AxonError::runtime_failure("run tool decision invalid"))
}

/// 连接或上游链路在产出任何模型输出之前失败：允许跨渠道续跑。
///
/// 只看失败类别，不看 HTTP 状态：中继把「上游 30 秒没回响应头」写成 504 响应体同样
/// 属于链路失败。带流内终态事件（例如 `response.failed`）或已产生可见输出的尝试
/// 都不在此列：前者是本 turn 已有上游结论，后者已经投影给用户。
fn is_resumable_channel_failure(outcome: &ProviderStreamingTransportOutcome) -> bool {
    let ProviderStreamingTransportOutcome::Error(error) = outcome else {
        return false;
    };
    error.provider_event_type.is_none()
        && matches!(
            error.failure_kind,
            axon_agent_runtime::ProviderStreamingTransportFailureKind::Timeout
                | axon_agent_runtime::ProviderStreamingTransportFailureKind::Connection
                | axon_agent_runtime::ProviderStreamingTransportFailureKind::Upstream
        )
}

/// 连接或上游链路在产出任何模型输出之前就失败时，生成跨渠道续跑现场。
///
/// 判定不看 HTTP 状态：中继把「上游 30 秒没回响应头」写成 504 响应体同样属于链路失败，
/// 关键是本 turn 没有产出正文与工具调用（`provider_event_type` 为空即没有流内终态事件，
/// 带可见输出的尝试已被 `VisibleOutput` 包装排除）。此时从该 turn 的历史续跑不会重放副作用。
fn connect_failure_resume_state(
    outcome: &ProviderStreamingTransportOutcome,
    transport_request: &ProviderStreamingTransportRequest,
    state: &PartialToolCallState<'_>,
) -> Option<ChannelResumeState> {
    if !is_resumable_channel_failure(outcome) {
        return None;
    }
    Some(ChannelResumeState {
        messages: transport_request.attempt.messages.clone(),
        tool_receipts: state.tool_receipts.borrow().clone(),
        tool_calls: state.tool_calls.borrow().clone(),
        reasoning: state.reasoning.borrow().clone(),
        turn_events: state.turn_events.borrow().clone(),
    })
}

/// 把续跑现场里已经落地的本轮事实并回响应，保证换渠道续跑后交付物不丢上一段成果。
fn prepend_resume_state(response: &mut RunResponse, state: &ChannelResumeState) {
    if !state.tool_receipts.is_empty() {
        let mut tool_receipts = state.tool_receipts.clone();
        tool_receipts.append(&mut response.tool_receipts);
        response.tool_receipts = tool_receipts;
    }
    if !state.tool_calls.is_empty() {
        let mut tool_calls = state.tool_calls.clone();
        tool_calls.append(&mut response.tool_calls);
        response.tool_calls = tool_calls;
    }
    if !state.reasoning.is_empty() {
        let mut reasoning = state.reasoning.clone();
        reasoning.append(&mut response.reasoning_messages);
        response.reasoning_messages = reasoning;
    }
    if !state.turn_events.is_empty() {
        let mut turn_events = state.turn_events.clone();
        turn_events.append(&mut response.turn_events);
        response.turn_events = turn_events;
    }
}
