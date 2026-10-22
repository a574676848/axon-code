//! Durable sequence 驱动的 wire 事件终态投影。
//!
//! 终态投影与实时投影共享同一份 durable 顺序：这里按 wire 事件的 sequence 顺序单遍推进，
//! 每条投影行落在其 durable 事实最终成立的位置上，不再按类别重新分组。
//! 事件身份由 payload 自有的稳定 part id 承载，因此重放对 reducer 是幂等的。

use std::collections::{HashMap, HashSet};

use axon_display::normalize_markdown_source;
use axon_protocol::wire::{
    WireAssistantMessage, WireEventEnvelope, WireEventKind, WireReasoningDelta,
};
use axon_tui_runtime::{AgentTuiMessagePart, AgentTuiReasoningLine, AgentTuiRunLine};

use super::run_output_assistant_renderer::project_assistant_content;
use super::run_output_generation_renderer::RetryGenerationProjection;
use super::run_output_memory_renderer::memory_recall_line_for;
use super::run_output_patch_renderer::{wire_file_change_diff, wire_file_change_key};
use super::run_output_plan_renderer::wire_plan_line;
use super::run_output_renderer::{TuiChannelAttempt, run_error_from_failure};
use super::run_output_review_renderer::wire_review_line;
use super::run_output_tool_renderer::{WireToolReceiptFilter, wire_usage_line};

const MEMORY_RECALL_SUBJECT: &str = "recall";

/// 把 durable 有序的 wire 事件投影为同样有序的终态行。
pub(super) fn wire_sequence_lines(
    events: &[WireEventEnvelope],
    channel_attempts: &[TuiChannelAttempt],
) -> Vec<AgentTuiRunLine> {
    let index = WireSequenceIndex::build(events);
    let mut projection = WireSequenceProjection::new(&index, channel_attempts);
    for (position, event) in events.iter().enumerate() {
        projection.observe(position, event);
    }
    projection.finish()
}

/// 与顺序无关的索引：只回答“某条 durable 事实最终在哪一帧成立”。
struct WireSequenceIndex {
    assistant_last: HashMap<String, usize>,
    reasoning_last: HashMap<String, usize>,
    usage_last: Option<usize>,
    plan_last: Option<usize>,
    memory_recall_last: Option<usize>,
    review_last: HashMap<String, usize>,
    superseded_file_changes: HashSet<usize>,
    cancelled_assistant_parts: HashSet<String>,
    cancelled_reasoning_parts: HashSet<String>,
    tool_receipts: WireToolReceiptFilter,
}

impl WireSequenceIndex {
    fn build(events: &[WireEventEnvelope]) -> Self {
        let mut index = Self {
            assistant_last: HashMap::new(),
            reasoning_last: HashMap::new(),
            usage_last: None,
            plan_last: None,
            memory_recall_last: None,
            review_last: HashMap::new(),
            superseded_file_changes: HashSet::new(),
            cancelled_assistant_parts: HashSet::new(),
            cancelled_reasoning_parts: HashSet::new(),
            tool_receipts: WireToolReceiptFilter::build(events),
        };
        let mut file_change_last = HashMap::new();
        for (position, event) in events.iter().enumerate() {
            match &event.kind {
                WireEventKind::AssistantMessage(message) => {
                    index
                        .assistant_last
                        .insert(message.part_id.clone(), position);
                }
                WireEventKind::ReasoningStart(start) => {
                    index.reasoning_last.insert(start.part_id.clone(), position);
                }
                WireEventKind::ReasoningDelta(delta) => {
                    index.reasoning_last.insert(delta.part_id.clone(), position);
                }
                WireEventKind::ReasoningEnd(end) => {
                    index.reasoning_last.insert(end.part_id.clone(), position);
                }
                WireEventKind::UsageUpdate(_) => index.usage_last = Some(position),
                WireEventKind::PlanUpdate(_) => index.plan_last = Some(position),
                WireEventKind::MemoryStatus(status) if status.subject == MEMORY_RECALL_SUBJECT => {
                    index.memory_recall_last = Some(position);
                }
                WireEventKind::ReviewStatus(review) => {
                    index.review_last.insert(review.part_id.clone(), position);
                }
                WireEventKind::FileChange(change) => {
                    let key = wire_file_change_key(change);
                    let superseded = match file_change_last.get(&key) {
                        Some((sequence, previous)) if *sequence <= event.sequence => {
                            index.superseded_file_changes.insert(*previous);
                            false
                        }
                        Some(_) => true,
                        None => false,
                    };
                    if superseded {
                        index.superseded_file_changes.insert(position);
                    } else {
                        file_change_last.insert(key, (event.sequence, position));
                    }
                }
                _ => {}
            }
        }
        index.collect_cancelled_parts(events);
        index
    }

    /// Provider 重试会撤销当前 generation 尚未提交的 part，重放不得复活它们。
    fn collect_cancelled_parts(&mut self, events: &[WireEventEnvelope]) {
        let mut assistant_generation = RetryGenerationProjection::default();
        let mut reasoning_generation = RetryGenerationProjection::default();
        for event in events {
            if let Some(failed) = assistant_generation.failed_parts_for_event(event) {
                self.cancelled_assistant_parts.extend(failed);
            }
            if let Some(failed) = reasoning_generation.failed_parts_for_event(event) {
                self.cancelled_reasoning_parts.extend(failed);
            }
            match &event.kind {
                WireEventKind::AssistantMessage(message) if message.delta => {
                    assistant_generation.observe_part(&message.part_id);
                    self.cancelled_assistant_parts.remove(&message.part_id);
                }
                WireEventKind::ReasoningDelta(delta) => {
                    reasoning_generation.observe_part(&delta.part_id);
                    self.cancelled_reasoning_parts.remove(&delta.part_id);
                }
                WireEventKind::ReasoningEnd(end) => reasoning_generation.commit_part(&end.part_id),
                _ => {}
            }
        }
    }
}

struct WireSequenceProjection<'a> {
    index: &'a WireSequenceIndex,
    channel_attempts: &'a [TuiChannelAttempt],
    lines: Vec<AgentTuiRunLine>,
    assistant: HashMap<String, AssistantAccumulator>,
    reasoning: HashMap<String, String>,
    reasoning_order: Vec<String>,
}

#[derive(Default)]
struct AssistantAccumulator {
    delta_content: String,
    final_content: Option<String>,
}

impl AssistantAccumulator {
    fn into_line(self, part_id: String) -> Option<AgentTuiRunLine> {
        let content = self
            .final_content
            .as_deref()
            .unwrap_or(self.delta_content.as_str());
        project_assistant_content(content).map(|content| {
            AgentTuiRunLine::Message(AgentTuiMessagePart::assistant(part_id, content))
        })
    }
}

impl<'a> WireSequenceProjection<'a> {
    fn new(index: &'a WireSequenceIndex, channel_attempts: &'a [TuiChannelAttempt]) -> Self {
        Self {
            index,
            channel_attempts,
            lines: Vec::new(),
            assistant: HashMap::new(),
            reasoning: HashMap::new(),
            reasoning_order: Vec::new(),
        }
    }

    fn observe(&mut self, position: usize, event: &WireEventEnvelope) {
        match &event.kind {
            WireEventKind::AssistantMessage(message) => {
                self.observe_assistant(position, message);
            }
            WireEventKind::ReasoningDelta(delta) => self.observe_reasoning_delta(position, delta),
            WireEventKind::ReasoningEnd(end) => self.finish_reasoning(position, &end.part_id),
            WireEventKind::UsageUpdate(usage) => {
                if self.index.usage_last == Some(position) {
                    self.lines
                        .push(AgentTuiRunLine::Usage(wire_usage_line(event, *usage)));
                }
            }
            WireEventKind::PlanUpdate(plan) => {
                if self.index.plan_last == Some(position) {
                    self.lines.push(AgentTuiRunLine::Plan(wire_plan_line(plan)));
                }
            }
            WireEventKind::MemoryStatus(status) => {
                if self.index.memory_recall_last == Some(position)
                    && let Some(line) = memory_recall_line_for(status)
                {
                    self.lines.push(line);
                }
            }
            WireEventKind::ReviewStatus(review) => {
                if self.index.review_last.get(&review.part_id) == Some(&position) {
                    self.lines
                        .push(AgentTuiRunLine::Review(wire_review_line(review)));
                }
            }
            WireEventKind::FileChange(change) => {
                if !self.index.superseded_file_changes.contains(&position)
                    && let Some(diff) = wire_file_change_diff(change)
                {
                    self.lines.push(AgentTuiRunLine::Diff(diff));
                }
            }
            WireEventKind::ToolResult(result) => {
                if let Some(receipt) = self.index.tool_receipts.receipt_for(result) {
                    self.lines.push(AgentTuiRunLine::ToolReceipt(receipt));
                }
            }
            WireEventKind::RuntimeError(failure) => {
                self.lines
                    .push(AgentTuiRunLine::Error(run_error_from_failure(
                        failure,
                        &event.event_id.0,
                        self.channel_attempts,
                    )));
            }
            _ => {}
        }
    }

    fn observe_assistant(&mut self, position: usize, message: &WireAssistantMessage) {
        if self
            .index
            .cancelled_assistant_parts
            .contains(&message.part_id)
        {
            return;
        }
        let accumulator = self.assistant.entry(message.part_id.clone()).or_default();
        if message.delta {
            accumulator.delta_content.push_str(&message.content);
        } else {
            accumulator.final_content = Some(message.content.clone());
        }
        if self.index.assistant_last.get(&message.part_id) != Some(&position) {
            return;
        }
        if let Some(accumulator) = self.assistant.remove(&message.part_id)
            && let Some(line) = accumulator.into_line(message.part_id.clone())
        {
            self.lines.push(line);
        }
    }

    fn observe_reasoning_delta(&mut self, position: usize, delta: &WireReasoningDelta) {
        let part_id = delta.part_id.trim();
        if part_id.is_empty() || delta.delta.trim().is_empty() {
            return;
        }
        if self.index.cancelled_reasoning_parts.contains(part_id) {
            return;
        }
        self.reasoning
            .entry(part_id.to_owned())
            .or_default()
            .push_str(&delta.delta);
        if !self.reasoning_order.iter().any(|id| id == part_id) {
            self.reasoning_order.push(part_id.to_owned());
        }
        if self.index.reasoning_last.get(part_id) == Some(&position) {
            self.emit_reasoning(part_id);
        }
    }

    fn finish_reasoning(&mut self, position: usize, part_id: &str) {
        if self.index.reasoning_last.get(part_id) == Some(&position) {
            self.emit_reasoning(part_id);
        }
    }

    fn emit_reasoning(&mut self, part_id: &str) {
        let Some(text) = self.reasoning.remove(part_id) else {
            return;
        };
        self.lines.push(AgentTuiRunLine::Reasoning(reasoning_line(
            part_id.to_owned(),
            &text,
        )));
    }

    /// durable 流未闭合的 reasoning part 仍然可见，收口顺序沿用首次出现顺序。
    fn finish(mut self) -> Vec<AgentTuiRunLine> {
        let trailing = std::mem::take(&mut self.reasoning_order)
            .into_iter()
            .filter_map(|part_id| {
                self.reasoning
                    .remove(&part_id)
                    .map(|text| AgentTuiRunLine::Reasoning(reasoning_line(part_id, &text)))
            })
            .collect::<Vec<_>>();
        self.lines.extend(trailing);
        self.lines
    }
}

fn reasoning_line(part_id: String, text: &str) -> AgentTuiReasoningLine {
    AgentTuiReasoningLine::new(part_id, normalize_markdown_source(text).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_protocol::wire::{
        WireAssistantMessage, WireCorrelationId, WireLifecycleStatus, WireReasoningEnd,
        WireReasoningStart, WireSchemaVersion, WireSessionRef, WireToolCall, WireToolResult,
        WireTurnLifecycle,
    };

    fn envelope(sequence: u64, kind: WireEventKind) -> WireEventEnvelope {
        WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(format!("event:{sequence}")),
            correlation_id: WireCorrelationId("turn".to_owned()),
            session: WireSessionRef {
                session_id: "session-1".to_owned(),
                run_id: Some("run-1".to_owned()),
                parent_session_id: None,
            },
            sequence,
            recorded_at_ms: i64::try_from(sequence).unwrap_or(i64::MAX),
            kind,
        }
    }

    fn kind_name(line: &AgentTuiRunLine) -> &'static str {
        match line {
            AgentTuiRunLine::Reasoning(_) => "reasoning",
            AgentTuiRunLine::Message(_) => "assistant",
            AgentTuiRunLine::ToolReceipt(_) => "tool",
            AgentTuiRunLine::Diff(_) => "diff",
            AgentTuiRunLine::Usage(_) => "usage",
            AgentTuiRunLine::Plan(_) => "plan",
            _ => "other",
        }
    }

    /// 终态投影必须按 durable sequence 推进，而不是按 reasoning/assistant/tool 等类别重新分组。
    #[test]
    fn wire_projection_follows_durable_sequence_across_categories() {
        let events = vec![
            envelope(
                1,
                WireEventKind::ReasoningStart(WireReasoningStart {
                    part_id: "reasoning-part:1".to_owned(),
                    reasoning_id: "provider-reasoning:1".to_owned(),
                }),
            ),
            envelope(
                2,
                WireEventKind::ReasoningDelta(WireReasoningDelta {
                    part_id: "reasoning-part:1".to_owned(),
                    reasoning_id: "provider-reasoning:1".to_owned(),
                    delta: "先分析".to_owned(),
                }),
            ),
            envelope(
                3,
                WireEventKind::ToolCall(WireToolCall {
                    part_id: "tool-part:1".to_owned(),
                    tool_call_id: "call-1".to_owned(),
                    request_id: None,
                    tool_name: "shell_command".to_owned(),
                    tool_kind: "shell".to_owned(),
                    summary: "cargo test".to_owned(),
                    arguments_summary: None,
                    intent: None,
                    execution_permission: None,
                }),
            ),
            envelope(
                4,
                WireEventKind::ToolResult(WireToolResult {
                    part_id: "tool-part:1".to_owned(),
                    tool_call_id: "call-1".to_owned(),
                    tool_name: Some("shell_command".to_owned()),
                    tool_kind: Some("shell".to_owned()),
                    status: WireLifecycleStatus::Completed,
                    summary: "测试通过".to_owned(),
                    output_preview: None,
                    stream: None,
                    sequence: None,
                    progress_phase: None,
                    items_completed: None,
                    items_total: None,
                    current_item: None,
                    duration_ms: None,
                    metadata: None,
                    error_type: None,
                    filesystem_write_performed: false,
                }),
            ),
            envelope(
                5,
                WireEventKind::ReasoningEnd(WireReasoningEnd {
                    part_id: "reasoning-part:1".to_owned(),
                    reasoning_id: "provider-reasoning:1".to_owned(),
                }),
            ),
            envelope(
                6,
                WireEventKind::AssistantMessage(WireAssistantMessage {
                    part_id: "assistant-part:1".to_owned(),
                    message_id: "assistant-message-final".to_owned(),
                    content: "最终回答".to_owned(),
                    delta: false,
                }),
            ),
            envelope(
                7,
                WireEventKind::UsageUpdate(axon_protocol::wire::WireUsageUpdate {
                    input_tokens: 10,
                    output_tokens: 5,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    reasoning_output_tokens: 0,
                    current_context_tokens: 15,
                    system_prompt_tokens: 0,
                    tool_definition_tokens: 0,
                    conversation_tokens: 0,
                }),
            ),
            envelope(
                8,
                WireEventKind::TurnLifecycle(WireTurnLifecycle {
                    part_id: "lifecycle:run".to_owned(),
                    turn_id: "run".to_owned(),
                    phase: axon_protocol::wire::WireLifecyclePhase::RegularTurn,
                    status: WireLifecycleStatus::Completed,
                    title: "Run completed".to_owned(),
                    kind: Some("RunLifecycle".to_owned()),
                    ui_phase: None,
                    tool_name: None,
                    attempt_index: None,
                    summary: None,
                    duration_ms: None,
                    started_at_ms: None,
                    total_tokens: None,
                    cache_hit_rate_basis_points: None,
                }),
            ),
        ];

        let lines = wire_sequence_lines(&events, &[]);
        let order = lines.iter().map(kind_name).collect::<Vec<_>>();

        assert_eq!(
            order,
            ["tool", "reasoning", "assistant", "usage"],
            "终态投影必须保持 durable 顺序：工具回执先于后闭合的 reasoning part"
        );
    }
}
