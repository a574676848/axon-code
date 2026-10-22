//! 统一 Agent turn runtime。

use axon_domain::{AxonError, AxonResult, TurnId};

#[cfg(test)]
use crate::ModelToolDefinition;
use crate::runtime_limits::PRE_FINAL_GUARD_MAX_DELAY_ROUNDS;
use crate::{
    AgentModelHistorySnapshot, AgentModelMessage, AgentModelMessageKind, AgentRuntimeNoteKind,
    AgentTurnSteerInput, ProviderStreamingTransportInput, ProviderStreamingTransportOutcome,
    ProviderStreamingTransportReport, ProviderStreamingTransportRequest,
    ProviderStreamingTransportResult, ProviderStreamingTransportRuntime,
    ProviderVisibleProjectionMetrics, TokenUsageTelemetry,
    estimated_history_tokens_for_provider_with_capabilities,
    insert_then_compress::{CompressionOutcome, WindowRolloverRuntime},
    project_steer_inputs, provider_streaming_prompt_cache_plan,
};

#[path = "agent_turn_runtime/provider_retry.rs"]
mod provider_retry;
#[path = "agent_turn_runtime/request_window.rs"]
mod request_window;
#[path = "agent_turn_runtime/tool_failure_loop_guard.rs"]
mod tool_failure_loop_guard;

#[cfg(test)]
#[path = "agent_turn_runtime/agent_turn_runtime_test_support.rs"]
mod test_support;

use provider_retry::*;
use tool_failure_loop_guard::ToolLoopGuard;

mod turn_id_serde {
    use axon_domain::TurnId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &TurnId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(value.0)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<TurnId, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(TurnId)
    }
}

#[path = "agent_turn_runtime/events.rs"]
mod events;
pub use events::{
    AgentTurnEvent, AgentTurnEventKind, agent_turn_event_part_id, agent_turn_provider_retry_part_id,
};

/// 单轮 provider turn 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnIterationReport {
    /// 当前 turn ID。
    pub turn_id: TurnId,
    /// provider transport 报告。
    pub transport_report: ProviderStreamingTransportReport,
}

/// 单轮工具阶段决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTurnToolDecision<TReceipt> {
    /// 工具已执行并生成下一轮消息。
    Continue {
        /// 当前轮工具回执。
        receipts: Vec<TReceipt>,
        /// 继续下一轮所需消息。
        next_messages: Vec<AgentModelMessage>,
    },
    /// 当前轮需要等待外部审批或外部动作。
    AwaitExternal {
        /// 当前轮已记录的工具回执。
        receipts: Vec<TReceipt>,
    },
}

/// 统一 turn runtime 最终报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnRuntimeReport<TReceipt> {
    /// 最终内容。
    pub content: String,
    /// HTTP 状态码。
    pub http_status: u16,
    /// token usage。
    pub token_usage: Option<TokenUsageTelemetry>,
    /// 工具回执。
    pub tool_receipts: Vec<TReceipt>,
    /// 按 turn 聚合的 provider 报告。
    pub turn_reports: Vec<AgentTurnIterationReport>,
    /// 统一 runtime 事件。
    pub turn_events: Vec<AgentTurnEvent>,
    /// 本轮任一 provider step 是否被输出 token 上限截断过。
    ///
    /// 一旦某一步被截断就**保持为真**（与 DSH 的粘性语义一致）：后一步即使正常
    /// 结束，也不能把「这一轮曾被预算掐断」这一事实抹掉，否则终态与退出原因
    /// 会随最后一步的运气而漂移。
    pub max_tokens_truncated: bool,
}

/// 当前 turn 在安全边界吸收运行中用户输入的来源。
pub trait AgentTurnSteerSource {
    /// 取出当前已排队输入，保持后续输入通道开启。
    fn drain(&self) -> Vec<AgentTurnSteerInput>;

    /// 原子关闭输入通道并取出最后一批输入。
    fn close_and_drain(&self) -> Vec<AgentTurnSteerInput>;

    /// 当前 turn 需要继续请求 provider 时重新开放输入通道。
    fn reopen(&self);

    /// 最终答复前仍未终态的后台作业数量（合同 §5.1.6 的 pre-final 守卫判据）。
    ///
    /// 默认 0：不承载后台作业的执行面无需感知作业注册表。
    fn pending_background_jobs(&self) -> usize {
        0
    }
}

/// 守卫提醒正文：给出数量与两条处理路径，不替模型决定该等还是该杀。
fn pre_final_guard_note(pending_jobs: usize) -> String {
    format!(
        "最终答复前仍有 {pending_jobs} 个后台作业未进入终态。请先处理它们再给出最终答复：\
         用 builtin.job_output 等待并读取结果，或对不再需要的作业用 builtin.job_kill 终止。\
         本条提醒至多延迟一轮；若确认无需等待，可直接给出最终答复。"
    )
}

/// 统一 Agent turn runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentTurnRuntime;

fn provider_generated_prefix_len(messages: &[AgentModelMessage]) -> usize {
    messages
        .iter()
        .rposition(|message| {
            message
                .provider_metadata
                .as_ref()
                .and_then(|metadata| metadata.tool_calls.as_ref())
                .is_some()
        })
        .map_or(0, |index| index.saturating_add(1))
}

impl AgentTurnRuntime {
    /// 执行统一 provider turn loop，CLI/UI 只需提供 transport 与工具执行适配。
    ///
    /// `compression` 提供循环内 insert-then-compress 上下文压缩；`None` 表示禁用。
    /// `input.turn_id` 是本轮 loop 首个 turn 的序号（`TurnId(0)` 表示未指定，按 1 起编号）：
    /// failover 会在同一个 run 内重跑整个 loop，每次尝试必须分配到不重叠的起点，由 turn
    /// 序号派生的 part 身份（assistant/reasoning/lifecycle）才不会跨尝试碰撞。
    pub fn run<TTransport, TExecuteTools, TReceipt>(
        &self,
        input: &ProviderStreamingTransportInput,
        mut transport: TTransport,
        mut execute_tools: TExecuteTools,
        steer_source: Option<&dyn AgentTurnSteerSource>,
        mut compression: Option<WindowRolloverRuntime<'_>>,
    ) -> AxonResult<AgentTurnRuntimeReport<TReceipt>>
    where
        TTransport: FnMut(&ProviderStreamingTransportRequest) -> ProviderStreamingTransportOutcome,
        TExecuteTools: FnMut(
            &ProviderStreamingTransportReport,
            &AgentModelHistorySnapshot,
            TurnId,
        ) -> AxonResult<AgentTurnToolDecision<TReceipt>>,
    {
        let mut request_input = input.clone();
        let mut messages = request_input.messages.clone();
        let mut tool_receipts = Vec::new();
        let mut turn_reports = Vec::new();
        let mut turn_events = Vec::new();
        let mut token_usage_total = TokenUsageTelemetry::default();
        let mut current_context_tokens = None;
        let mut provider_accounted_message_count = None;
        let mut previous_cache_shape = None;
        let mut pending_projection_metrics: Option<ProviderVisibleProjectionMetrics> = None;
        let first_turn_id = input.turn_id.0.max(1);
        let mut turn_index = 0_u32;
        let mut tool_loop_guard = ToolLoopGuard::default();
        let mut history_window_id: Option<String> = None;
        let mut window_changed = false;
        // 守卫的独立去重标记：与预算提醒的 `reminder_emitted` 无关，
        // 共享标记会让其中一方被另一方静默吞掉（设计 §6.3）。
        let mut pre_final_guard_emitted = false;
        loop {
            turn_index = turn_index
                .checked_add(1)
                .ok_or(AxonError::runtime_failure("AgentTurnRuntime turn 数过大"))?;
            let turn_id = TurnId(
                first_turn_id
                    .saturating_add(u64::from(turn_index))
                    .saturating_sub(1),
            );
            let turn_start_ms = now_ms();
            let mut window_replaced = false;
            let estimated_context_tokens = context_tokens_for_next_request(
                &messages,
                current_context_tokens,
                provider_accounted_message_count,
                &request_input,
            );
            if let Some(active) = compression.as_mut() {
                let format = match request_input.protocol {
                    crate::ProviderStreamProtocol::OpenAiResponses => {
                        crate::PromptCacheProviderWireFormat::OpenAiResponses
                    }
                    crate::ProviderStreamProtocol::OpenAiCompatible => {
                        crate::PromptCacheProviderWireFormat::OpenAiCompatibleChat
                    }
                    crate::ProviderStreamProtocol::AnthropicMessages => {
                        crate::PromptCacheProviderWireFormat::AnthropicMessages
                    }
                };
                active.runtime.configure_provider_wire(
                    format,
                    &request_input.model_tools,
                    request_input.supports_vision,
                );
                active.runtime.refresh_rollover_markers(&messages);
                let explicit_rollover = messages.iter().any(|message| {
                    matches!(
                        message.kind,
                        AgentModelMessageKind::RuntimeNote(
                            AgentRuntimeNoteKind::ContextRolloverRequest
                        )
                    )
                });
                let compression_outcome = if explicit_rollover {
                    active.maybe_compress_requested(
                        &mut messages,
                        estimated_context_tokens,
                        turn_id,
                        &mut turn_events,
                    )
                } else {
                    active.maybe_compress(
                        &mut messages,
                        estimated_context_tokens,
                        turn_id,
                        &mut turn_events,
                    )
                };
                match compression_outcome {
                    CompressionOutcome::Compressed => {
                        request_input.server_conversation_state_ref = None;
                        window_changed = true;
                        window_replaced = true;
                        pending_projection_metrics = active.take_projection_metrics();
                    }
                    // fallback runtime note 已限制 agent 先写便签再调用 new_context；
                    // 这里必须继续发送该提示，不能把可恢复状态升级为进程错误。
                    CompressionOutcome::Prompted => {}
                    CompressionOutcome::Idle => {}
                }
                history_window_id = active.rollover.current_window_id();
                if window_changed && !window_replaced {
                    publish_active_window_best_effort(active, &mut messages);
                }
            }
            request_input.messages = messages
                .clone()
                .with_history_window_id(history_window_id.clone());
            request_input.turn_id = turn_id;
            refresh_prompt_cache_plan(&mut request_input)?;
            // 上下文构成随每次真实请求刷新：读数对应即将发出的这一份 payload，
            // 并随请求一起交给传输层，由该次请求的用量帧同步投影给客户端。
            // 成本已在估算器里实测（1.4MB 历史约 1.1ms，release），相对一次 Provider
            // 往返可忽略；这里刻意不做增量缓存，避免历史被压缩重写后读数失真。
            request_input.context_composition =
                Some(crate::token_estimator::estimate_context_composition(
                    &request_input.messages,
                    provider_wire_format(request_input.protocol),
                    &request_input.model_tools,
                    request_input.supports_vision,
                ));

            turn_events.push(
                AgentTurnEvent::new(
                    turn_id,
                    AgentTurnEventKind::TurnStarted,
                    format!("provider turn {} started", turn_id.0),
                )
                .with_phase("provider"),
            );

            let ProviderStreamingTransportResult {
                report,
                final_messages,
            } = request_window::WindowRequest {
                input: &request_input,
            }
            .send(&mut transport)?;

            let turn_duration = now_ms().saturating_sub(turn_start_ms);
            append_provider_retry_events(
                &mut turn_events,
                turn_id,
                &report,
                &request_input.model_id,
            );
            current_context_tokens = report
                .runner_report
                .token_usage
                .map(|token_usage| token_usage.current_context_tokens)
                .filter(|tokens| *tokens > 0);
            if let Some(token_usage) = report.runner_report.token_usage {
                token_usage_total.add_assign(token_usage);
            }
            if request_input
                .capabilities
                .supports_server_conversation_state
                && request_input.protocol == crate::ProviderStreamProtocol::OpenAiResponses
                && report.runner_report.server_conversation_state_ref.is_some()
            {
                request_input.server_conversation_state_ref =
                    report.runner_report.server_conversation_state_ref.clone();
            }

            let cache_shape_comparison = report.cache_shape.compare(previous_cache_shape.as_ref());
            let cache_shape_metadata = report.cache_shape.event_metadata_with_projection(
                &cache_shape_comparison,
                pending_projection_metrics.take(),
            );
            previous_cache_shape = Some(report.cache_shape.clone());
            turn_events.push(
                AgentTurnEvent::new(
                    turn_id,
                    AgentTurnEventKind::ProviderCompleted,
                    format!(
                        "provider completed: http_status={} tool_calls={} end_turn={:?}",
                        report.runner_report.http_status,
                        report.runner_report.tool_calls.len(),
                        report.runner_report.end_turn,
                    ),
                )
                .with_duration_ms(turn_duration)
                .with_phase("provider")
                .with_metadata(cache_shape_metadata),
            );
            if report.runner_report.tool_calls.is_empty() {
                messages = final_messages;
                let steered_inputs = steer_source
                    .map(AgentTurnSteerSource::close_and_drain)
                    .unwrap_or_default();
                if !steered_inputs.is_empty() {
                    if !report.runner_report.content.trim().is_empty() {
                        messages.make_mut().push(AgentModelMessage::text(
                            "assistant",
                            report.runner_report.content.clone(),
                        ));
                    }
                    let provider_accounted_prefix = messages.len();
                    let steered_messages =
                        project_steer_inputs(steered_inputs, &messages, &request_input);
                    messages.make_mut().extend(steered_messages);
                    provider_accounted_message_count =
                        current_context_tokens.map(|_| provider_accounted_prefix);
                    if let Some(source) = steer_source {
                        source.reopen();
                    }
                    turn_reports.push(AgentTurnIterationReport {
                        turn_id,
                        transport_report: report,
                    });
                    continue;
                }
                let content = report.runner_report.content.clone();
                if report.runner_report.end_turn == Some(false) {
                    if !content.trim().is_empty() {
                        messages
                            .make_mut()
                            .push(AgentModelMessage::text("assistant", content));
                    }
                    provider_accounted_message_count =
                        current_context_tokens.map(|_| messages.len());
                    turn_reports.push(AgentTurnIterationReport {
                        turn_id,
                        transport_report: report,
                    });
                    continue;
                }
                // pre-final 守卫（合同 §5.1.6）：存在未终态后台作业时先提醒一次，
                // 避免最终答复丢掉还没回收的结果。至多延迟一轮，不改变作业自身生命周期。
                let pending_jobs =
                    steer_source.map_or(0, AgentTurnSteerSource::pending_background_jobs);
                if pending_jobs > 0
                    && !pre_final_guard_emitted
                    && PRE_FINAL_GUARD_MAX_DELAY_ROUNDS > 0
                {
                    pre_final_guard_emitted = true;
                    if !content.trim().is_empty() {
                        messages
                            .make_mut()
                            .push(AgentModelMessage::text("assistant", content));
                    }
                    messages.make_mut().push(AgentModelMessage::runtime_note(
                        AgentRuntimeNoteKind::PreFinalGuard,
                        pre_final_guard_note(pending_jobs),
                    ));
                    provider_accounted_message_count =
                        current_context_tokens.map(|_| messages.len());
                    turn_events.push(
                        AgentTurnEvent::new(
                            turn_id,
                            AgentTurnEventKind::Completed,
                            "pre-final guard deferred the final answer",
                        )
                        .with_phase("guard"),
                    );
                    turn_reports.push(AgentTurnIterationReport {
                        turn_id,
                        transport_report: report,
                    });
                    continue;
                }
                turn_events.push(
                    AgentTurnEvent::new(
                        turn_id,
                        AgentTurnEventKind::Completed,
                        "provider turn finished without additional tool calls",
                    )
                    .with_phase("completion"),
                );
                let http_status = report.runner_report.http_status;
                if window_changed && let Some(active) = compression.as_mut() {
                    let mut final_history = messages.clone();
                    final_history.push(AgentModelMessage::text("assistant", content.clone()));
                    publish_active_window_best_effort(active, &mut final_history);
                }
                turn_reports.push(AgentTurnIterationReport {
                    turn_id,
                    transport_report: report,
                });
                let max_tokens_truncated = turn_reports.iter().any(|iteration| {
                    iteration
                        .transport_report
                        .runner_report
                        .max_tokens_truncated
                });
                return Ok(AgentTurnRuntimeReport {
                    content,
                    http_status,
                    token_usage: token_usage_option(token_usage_total),
                    tool_receipts,
                    turn_reports,
                    turn_events,
                    max_tokens_truncated,
                });
            }
            match execute_tools(&report, &final_messages, turn_id)? {
                AgentTurnToolDecision::Continue {
                    receipts,
                    next_messages,
                    ..
                } => {
                    let mut next_messages = next_messages;
                    if let Some(warning) = tool_loop_guard.observe(&next_messages)? {
                        next_messages.push(AgentModelMessage::runtime_note(
                            AgentRuntimeNoteKind::ToolFeedback,
                            warning,
                        ));
                    }
                    tool_receipts.extend(receipts);
                    turn_events.push(
                        AgentTurnEvent::new(
                            turn_id,
                            AgentTurnEventKind::ToolResultsRecorded,
                            format!("tool receipts recorded: {}", tool_receipts.len()),
                        )
                        .with_phase("tool"),
                    );
                    let provider_accounted_prefix = final_messages
                        .len()
                        .saturating_add(provider_generated_prefix_len(&next_messages));
                    messages = final_messages;
                    messages.make_mut().extend(next_messages);
                    provider_accounted_message_count =
                        current_context_tokens.map(|_| provider_accounted_prefix);
                    if let Some(source) = steer_source {
                        let inputs = source.drain();
                        let projected = project_steer_inputs(inputs, &messages, &request_input);
                        messages.make_mut().extend(projected);
                    }
                    turn_reports.push(AgentTurnIterationReport {
                        turn_id,
                        transport_report: report,
                    });
                }
                AgentTurnToolDecision::AwaitExternal { receipts } => {
                    if window_changed && let Some(active) = compression.as_mut() {
                        publish_active_window_best_effort(active, &mut messages);
                    }
                    tool_receipts.extend(receipts);
                    turn_events.push(
                        AgentTurnEvent::new(
                            turn_id,
                            AgentTurnEventKind::AwaitingExternal,
                            "turn runtime 已等待外部审批或外部动作",
                        )
                        .with_phase("external"),
                    );
                    let content = report.runner_report.content.clone();
                    let http_status = report.runner_report.http_status;
                    turn_reports.push(AgentTurnIterationReport {
                        turn_id,
                        transport_report: report,
                    });
                    let max_tokens_truncated = turn_reports.iter().any(|iteration| {
                        iteration
                            .transport_report
                            .runner_report
                            .max_tokens_truncated
                    });
                    return Ok(AgentTurnRuntimeReport {
                        content,
                        http_status,
                        token_usage: token_usage_option(token_usage_total),
                        tool_receipts,
                        turn_reports,
                        turn_events,
                        max_tokens_truncated,
                    });
                }
            }
        }
    }
}

fn publish_active_window_best_effort(
    active: &mut WindowRolloverRuntime<'_>,
    messages: &mut AgentModelHistorySnapshot,
) {
    if active.rollover.publish_active_window(messages).is_err() {
        active.runtime.emit_fallback(messages);
    }
}

#[cfg(test)]
#[path = "agent_turn_runtime/agent_turn_runtime_tests.rs"]
mod agent_turn_runtime_tests;

#[cfg(test)]
#[path = "agent_turn_runtime/agent_turn_runtime_verify_continue_tests.rs"]
mod agent_turn_runtime_verify_continue_tests;
