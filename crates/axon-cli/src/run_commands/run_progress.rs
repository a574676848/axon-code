//! `axon run` 运行进度事件。

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use axon_agent_runtime::{ContextTokenComposition, TokenUsageTelemetry};
use axon_domain::{UserAnswerItem, UserQuestionItem};
use axon_protocol::wire::{
    WireAssistantMessage, WireContextRollover, WireCorrelationId, WireEventEnvelope, WireEventKind,
    WireFileChangeEvent, WireLifecyclePhase, WireLifecycleStatus, WireProviderStatus,
    WireSchemaVersion, WireSessionRef, WireToolCall, WireToolResult, WireTurnLifecycle,
    WireUsageUpdate, WireUserMessage, WireUserQuestion,
};

use crate::tool_orchestrator::{
    ToolExecutionRequest, ToolOutputChunk, ToolReceipt, ToolRuntimeEvent,
};

use super::run_file_change_projection::diff_part_id;
use super::run_plan_projection::plan_update_from_tool_receipt;
use super::run_progress_apply_patch_snapshot::apply_patch_diff_snapshot_from_execution;

const KIND_STREAMING_STARTED: &str = axon_protocol::wire::WIRE_LIFECYCLE_KIND_STREAMING_STARTED;
pub(crate) const KIND_STREAMING_COMPLETED: &str =
    axon_protocol::wire::WIRE_LIFECYCLE_KIND_STREAMING_COMPLETED;
const STEER_USER_MESSAGE_ID_PREFIX: &str = "steer-user-message";
/// 问答事件的稳定 part ID 前缀。
const QUESTION_PART_ID_PREFIX: &str = "part:question:";
/// 问答事件的稳定关联 ID 前缀。
const QUESTION_CORRELATION_PREFIX: &str = "question:";
static RUN_INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// 问答事件的稳定 part ID：同一问题的多次状态变化落在同一个展示单元上。
#[must_use]
pub(crate) fn question_part_id(call_id: &str) -> String {
    format!("{QUESTION_PART_ID_PREFIX}{call_id}")
}

/// 问答事件的稳定关联 ID。
#[must_use]
pub(crate) fn question_correlation(call_id: &str) -> String {
    format!("{QUESTION_CORRELATION_PREFIX}{call_id}")
}

/// 构造问答事件的 wire 载荷：实时投影与桌面、TUI 消费共用唯一构造点。
#[must_use]
pub(crate) fn question_wire_payload(
    part_id: &str,
    call_id: &str,
    status: &str,
    questions: &[UserQuestionItem],
    answers: &[UserAnswerItem],
    handoff: Option<&str>,
    deadline_at_ms: i64,
) -> WireUserQuestion {
    WireUserQuestion {
        part_id: part_id.to_owned(),
        call_id: call_id.to_owned(),
        status: status.to_owned(),
        questions: questions.to_vec(),
        answers: answers.to_vec(),
        handoff: handoff.map(ToOwned::to_owned),
        deadline_at_ms,
    }
}

#[path = "run_progress_models.rs"]
mod run_progress_models;
#[path = "run_progress_reasoning.rs"]
mod run_progress_reasoning;
#[path = "run_progress_stream_normalizer.rs"]
mod run_progress_stream_normalizer;
#[path = "run_progress_tool_budget.rs"]
mod run_progress_tool_budget;
#[path = "run_progress_views.rs"]
mod run_progress_views;
#[path = "run_progress_wire.rs"]
mod run_progress_wire;

pub(crate) use run_progress_models::{
    RunLifecycleStatusView, RunToolCompletionView, RunToolDiffSnapshotView, RunToolExecutionView,
    RunToolOutputChunk, RunToolProgressView,
};
pub(super) use run_progress_stream_normalizer::RunProgressStreamNormalizer;
use run_progress_tool_budget::ToolResultBudget;
use run_progress_views::{
    tool_completion_view_from_receipt, tool_execution_view_from_execution, tool_output_chunk,
    tool_progress_view,
};
use run_progress_wire::{
    current_time_ms, execution_permission_name, lifecycle_correlation, lifecycle_phase,
    streaming_summary, tool_correlation, wire_lifecycle_status,
};

/// `axon run` 内部进度的 wire 事实源事件。
#[derive(Debug)]
pub(crate) struct RunProgressWireEvent {
    pub(crate) event: WireEventEnvelope,
}

/// 可跨线程共享的运行事件总线入口。
pub(crate) type RunProgressEventBus =
    Arc<dyn Fn(RunProgressWireEvent) -> Result<(), String> + Send + Sync>;

/// 同步运行进度事件接收器。
#[derive(Clone)]
pub(super) struct RunProgressSink<'a> {
    emit: Rc<RefCell<&'a mut dyn FnMut(RunProgressWireEvent)>>,
    session: WireSessionRef,
    next_sequence: Rc<Cell<u64>>,
    emitted_reasoning_text: Rc<RefCell<BTreeMap<String, String>>>,
    ended_reasoning_parts: Rc<RefCell<BTreeSet<String>>>,
    emitted_assistant_text: Rc<RefCell<BTreeMap<String, String>>>,
    /// 本 run 内下一次渠道尝试可用的首个 provider turn 序号。
    ///
    /// 渠道 failover 会重跑整个 provider turn loop；turn 序号在该 run 内必须全局唯一，
    /// 否则新尝试会复用上一尝试的 assistant/reasoning part 身份，把两段正文拼进同一个
    /// 展示单元与同一条失败诊断记录。
    provider_turn_epoch: Rc<Cell<u64>>,
    /// 工具结果进入事件流前的预算裁定。
    tool_budget: ToolResultBudget,
    /// 本次渠道尝试的上下文构成估算：请求侧估算一次、整次尝试不变。
    ///
    /// 它搭在每一条用量事件上投出，实时帧与收尾补发的最终用量才完全同形，
    /// 重复投递也能按值识别出来。
    attempt_composition: Rc<Cell<Option<ContextTokenComposition>>>,
    /// 本次尝试已投出的最后一份用量快照。
    ///
    /// 用量帧是该条 provider 流的累计快照，流内重复投同一份没有信息量；
    /// 收尾补发（有些 Provider 只在聚合结果里给 usage）命中同一份时在这里被吸收。
    last_usage: Rc<RefCell<Option<WireUsageUpdate>>>,
}

impl<'a> RunProgressSink<'a> {
    pub(super) fn with_session_id(
        emit: &'a mut dyn FnMut(RunProgressWireEvent),
        session_id: String,
    ) -> Self {
        Self {
            emit: Rc::new(RefCell::new(emit)),
            session: WireSessionRef {
                session_id,
                run_id: Some(format!(
                    "run-{}-{}",
                    current_time_ms(),
                    RUN_INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed)
                )),
                parent_session_id: None,
            },
            next_sequence: Rc::new(Cell::new(1)),
            emitted_reasoning_text: Rc::new(RefCell::new(BTreeMap::new())),
            ended_reasoning_parts: Rc::new(RefCell::new(BTreeSet::new())),
            emitted_assistant_text: Rc::new(RefCell::new(BTreeMap::new())),
            provider_turn_epoch: Rc::new(Cell::new(1)),
            tool_budget: ToolResultBudget::default(),
            attempt_composition: Rc::new(Cell::new(None)),
            last_usage: Rc::new(RefCell::new(None)),
        }
    }

    /// 绑定运行态归档根：超预算的工具结果全文落在这里，事件只带路径。
    #[must_use]
    pub(super) fn with_tool_result_archive_root(
        mut self,
        archive_root: std::path::PathBuf,
    ) -> Self {
        self.tool_budget = ToolResultBudget::with_archive_root(archive_root);
        self
    }

    pub(super) fn emit(&self, event: RunProgressWireEvent) {
        (self.emit.borrow_mut())(event);
    }

    /// 取本次渠道尝试的首个 provider turn 序号。
    pub(super) fn begin_provider_attempt(&self) -> u64 {
        self.provider_turn_epoch.get()
    }

    /// 记录本次渠道尝试已消费到的 turn 序号，供下一次尝试接着编号。
    pub(super) fn observe_provider_turn(&self, turn_id: u64) {
        let next = turn_id.saturating_add(1);
        if next > self.provider_turn_epoch.get() {
            self.provider_turn_epoch.set(next);
        }
    }

    pub(super) fn provider_status(&self, status: WireProviderStatus) {
        self.emit_kind(
            format!("provider:{}:{}", status.provider, status.model),
            WireEventKind::ProviderStatus(status),
        );
    }

    pub(super) fn streaming_started(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
        attempt_index: usize,
    ) {
        // 新的一条 provider 流开始：上一条流的用量快照序列到此为止，
        // 即使两条流的数值恰好相同，也必须各自投一次。
        self.last_usage.replace(None);
        let provider = provider.into();
        let model = model.into();
        self.emit_kind(
            format!("stream:{provider}:{model}:{attempt_index}"),
            WireEventKind::TurnLifecycle(WireTurnLifecycle {
                part_id: format!("generation:stream:{provider}:{model}:{attempt_index}"),
                turn_id: "provider-stream".to_owned(),
                phase: WireLifecyclePhase::RegularTurn,
                status: WireLifecycleStatus::Started,
                title: "生成".to_owned(),
                kind: Some(KIND_STREAMING_STARTED.to_owned()),
                ui_phase: Some("generation".to_owned()),
                tool_name: (!model.trim().is_empty()).then_some(model.clone()),
                attempt_index: u64::try_from(attempt_index).ok(),
                summary: Some(streaming_summary(&provider, &model)),
                duration_ms: None,
                started_at_ms: None,
                total_tokens: None,
                cache_hit_rate_basis_points: None,

                warnings: Vec::new(),
            }),
        );
    }

    pub(super) fn streaming_completed(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
        attempt_index: usize,
    ) {
        let provider = provider.into();
        let model = model.into();
        self.emit_kind(
            format!("stream-completed:{provider}:{model}:{attempt_index}"),
            WireEventKind::TurnLifecycle(WireTurnLifecycle {
                part_id: format!("generation:stream:{provider}:{model}:{attempt_index}"),
                turn_id: "provider-stream".to_owned(),
                phase: WireLifecyclePhase::RegularTurn,
                status: WireLifecycleStatus::Completed,
                title: "生成".to_owned(),
                kind: Some(KIND_STREAMING_COMPLETED.to_owned()),
                ui_phase: Some("generation".to_owned()),
                tool_name: (!model.trim().is_empty()).then_some(model.clone()),
                attempt_index: u64::try_from(attempt_index).ok(),
                summary: Some(streaming_summary(&provider, &model)),
                duration_ms: None,
                started_at_ms: None,
                total_tokens: None,
                cache_hit_rate_basis_points: None,

                warnings: Vec::new(),
            }),
        );
    }

    pub(super) fn lifecycle_status(&self, view: RunLifecycleStatusView) {
        self.emit_kind(
            lifecycle_correlation(&view.kind, view.attempt_index),
            WireEventKind::TurnLifecycle(WireTurnLifecycle {
                part_id: view.id,
                turn_id: view.kind.clone(),
                phase: lifecycle_phase(&view.phase),
                status: view.status,
                title: view.title,
                kind: Some(view.kind),
                ui_phase: Some(view.phase),
                tool_name: view.tool_name,
                attempt_index: view
                    .attempt_index
                    .and_then(|value| u64::try_from(value).ok()),
                summary: Some(view.summary),
                duration_ms: view.duration_ms,
                started_at_ms: view.started_at_ms,
                total_tokens: view.total_tokens,
                cache_hit_rate_basis_points: view.cache_hit_rate_basis_points,

                warnings: Vec::new(),
            }),
        );
    }

    pub(super) fn context_rollover(&self, rollover: WireContextRollover) {
        self.emit_kind(
            format!("context-rollover:{}", rollover.window_id),
            WireEventKind::ContextRollover(rollover),
        );
    }

    pub(super) fn assistant_delta(
        &self,
        part_id: impl Into<String>,
        message_id: impl Into<String>,
        delta: impl Into<String>,
    ) {
        let part_id = part_id.into();
        let delta = delta.into();
        self.emitted_assistant_text
            .borrow_mut()
            .entry(part_id.clone())
            .or_default()
            .push_str(&delta);
        self.emit_kind(
            part_id.clone(),
            WireEventKind::AssistantMessage(WireAssistantMessage {
                part_id: part_id.clone(),
                message_id: message_id.into(),
                content: delta,
                delta: true,
            }),
        );
    }

    pub(super) fn partial_assistant_content(&self) -> String {
        self.emitted_assistant_text
            .borrow()
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(super) fn steered_user_message(&self, content: impl Into<String>) {
        let sequence = self.next_sequence.get();
        let message_id = format!("{STEER_USER_MESSAGE_ID_PREFIX}:{sequence}");
        self.emit_kind(
            message_id.clone(),
            WireEventKind::UserMessage(WireUserMessage {
                part_id: message_id.clone(),
                message_id,
                content: content.into(),
                local_files: Vec::new(),
            }),
        );
    }

    /// 绑定本次渠道尝试的上下文构成估算。
    ///
    /// 构成是请求侧的事实，收尾补发与实时帧都要带上它：两处同形之后，
    /// 同一条流的同一份用量才可能被识别为重复，而不是被当成两次不同的观测。
    pub(super) fn begin_attempt_composition(&self, composition: Option<ContextTokenComposition>) {
        self.attempt_composition.set(composition);
    }

    /// 投出一条用量观测；同一条流的同一份快照只投一次。
    pub(super) fn token_usage_updated(&self, usage: TokenUsageTelemetry) {
        let mut usage = usage;
        if !usage.has_context_composition()
            && let Some(composition) = self.attempt_composition.get()
        {
            usage.set_context_composition(composition);
        }
        let wire = WireUsageUpdate {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cache_read_input_tokens,
            cache_write_input_tokens: usage.cache_write_input_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
            current_context_tokens: usage.current_context_tokens,
            system_prompt_tokens: usage.system_prompt_tokens,
            tool_definition_tokens: usage.tool_definition_tokens,
            conversation_tokens: usage.conversation_tokens,
        };
        {
            let mut last = self.last_usage.borrow_mut();
            // 数值与构成都一样即同一份快照：再投一次只会让每个消费方各自防重。
            if last.as_ref() == Some(&wire) {
                return;
            }
            *last = Some(wire);
        }
        self.emit_kind("usage".to_owned(), WireEventKind::UsageUpdate(wire));
    }

    pub(super) fn tool_execution_started(&self, request: &ToolExecutionRequest) {
        let view = tool_execution_view_from_execution(request);
        self.emit_kind(
            tool_correlation(&view.provider_call_id),
            WireEventKind::ToolCall(WireToolCall {
                part_id: view.part_id.clone(),
                tool_call_id: view.provider_call_id.clone(),
                request_id: Some(view.request_id),
                tool_name: view.tool_name.clone(),
                tool_kind: view.kind.to_owned(),
                summary: "工具调用已开始".to_owned(),
                arguments_summary: Some(view.arguments_preview.clone()),
                intent: view.intent.clone(),
                execution_permission: Some(execution_permission_name(&view.execution_permission)),
            }),
        );
        if let Some(snapshot) = apply_patch_diff_snapshot_from_execution(request) {
            self.tool_diff_snapshot(snapshot);
        }
    }

    pub(super) fn tool_output_chunk(&self, chunk: ToolOutputChunk) {
        let chunk = tool_output_chunk(chunk);
        // 流式分片本身就是 durable 事件：不封顶一样能把一条订阅撑爆。
        let text = self.tool_budget.bound_chunk(&chunk.text);
        self.emit_kind(
            tool_correlation(&chunk.provider_call_id),
            WireEventKind::ToolResult(WireToolResult {
                part_id: chunk.part_id,
                tool_call_id: chunk.provider_call_id,
                tool_name: Some(chunk.tool_name),
                tool_kind: Some(chunk.kind.to_owned()),
                status: WireLifecycleStatus::Running,
                summary: text.clone(),
                output_preview: Some(text),
                stream: Some(chunk.stream.to_owned()),
                sequence: Some(chunk.sequence),
                progress_phase: None,
                items_completed: None,
                items_total: None,
                current_item: None,
                duration_ms: None,
                metadata: None,
                error_type: None,
                filesystem_write_performed: false,
            }),
        );
    }

    pub(super) fn tool_runtime_event(&self, event: ToolRuntimeEvent) {
        match event {
            ToolRuntimeEvent::Output(chunk) => self.tool_output_chunk(chunk),
            ToolRuntimeEvent::Progress(progress) => {
                self.tool_progress_updated(tool_progress_view(progress));
            }
        }
    }

    pub(super) fn tool_execution_completed(
        &self,
        execution_request: &ToolExecutionRequest,
        receipt: &ToolReceipt,
        duration_ms: Option<u64>,
    ) {
        let plan_update = plan_update_from_tool_receipt(
            &self.session.session_id,
            &receipt.status,
            receipt.metadata.as_ref(),
        );
        // 进事件流之前先过预算：正文头尾保留 + 全文归档，体积型 metadata 换计数。
        // 计划投影读的是原始回执，裁剪不影响它。
        let mut view = tool_completion_view_from_receipt(execution_request, receipt, duration_ms);
        view.summary = self
            .tool_budget
            .bound_summary(&receipt.summary, &receipt.tool_name);
        view.metadata = receipt.metadata.as_ref().map(|metadata| {
            self.tool_budget
                .bound_metadata(metadata, &receipt.tool_name)
        });
        self.emit_tool_execution_completed(view);
        if let Some(plan) = plan_update {
            self.emit_kind(plan.part_id.clone(), WireEventKind::PlanUpdate(plan));
        }
    }

    pub(super) fn consume_line(
        &self,
        normalizer: &mut RunProgressStreamNormalizer,
        line: &str,
    ) -> bool {
        normalizer.consume_line(line, self)
    }

    fn tool_diff_snapshot(&self, view: RunToolDiffSnapshotView) {
        for (path, change) in view.changes {
            let path_text = path.display().to_string();
            self.emit_kind(
                tool_correlation(&view.provider_call_id),
                WireEventKind::FileChange(WireFileChangeEvent {
                    part_id: diff_part_id(&view.tool_part_id, &path_text),
                    tool_part_id: Some(view.tool_part_id.clone()),
                    tool_call_id: Some(view.provider_call_id.clone()),
                    path: path_text,
                    change,
                }),
            );
        }
    }

    fn tool_progress_updated(&self, progress: RunToolProgressView) {
        self.emit_kind(
            tool_correlation(&progress.provider_call_id),
            WireEventKind::ToolResult(WireToolResult {
                part_id: progress.part_id,
                tool_call_id: progress.provider_call_id,
                tool_name: Some(progress.tool_name),
                tool_kind: Some(progress.kind.to_owned()),
                status: WireLifecycleStatus::Running,
                summary: progress
                    .message
                    .clone()
                    .unwrap_or_else(|| progress.phase.clone()),
                output_preview: progress.message,
                stream: None,
                sequence: None,
                progress_phase: Some(progress.phase),
                items_completed: progress.items_completed,
                items_total: progress.items_total,
                current_item: progress.current_item,
                duration_ms: progress.elapsed_ms,
                metadata: None,
                error_type: None,
                filesystem_write_performed: false,
            }),
        );
    }

    fn emit_tool_execution_completed(&self, view: RunToolCompletionView) {
        self.emit_kind(
            tool_correlation(&view.provider_call_id),
            WireEventKind::ToolResult(WireToolResult {
                part_id: view.part_id,
                tool_call_id: view.provider_call_id,
                tool_name: Some(view.tool_name),
                tool_kind: Some(view.kind.to_owned()),
                status: wire_lifecycle_status(&view.status),
                summary: view.summary,
                output_preview: None,
                stream: None,
                sequence: None,
                progress_phase: None,
                items_completed: None,
                items_total: None,
                current_item: None,
                duration_ms: view.duration_ms,
                metadata: view.metadata,
                error_type: view.error_type,
                filesystem_write_performed: view.filesystem_write_performed,
            }),
        );
    }

    fn emit_kind(&self, correlation_id: String, kind: WireEventKind) {
        let sequence = self.next_sequence.get();
        self.next_sequence.set(sequence.saturating_add(1));
        self.emit(RunProgressWireEvent {
            event: WireEventEnvelope {
                schema_version: WireSchemaVersion::default(),
                event_id: WireCorrelationId(format!("{}:{sequence}", self.session.session_id)),
                correlation_id: WireCorrelationId(correlation_id),
                session: self.session.clone(),
                sequence,
                recorded_at_ms: current_time_ms(),
                kind,
            },
        });
    }
}

#[cfg(test)]
#[path = "run_progress_failover_tests.rs"]
mod run_progress_failover_tests;
#[cfg(test)]
#[path = "run_progress_plan_tests.rs"]
mod run_progress_plan_tests;
#[cfg(test)]
#[path = "run_progress_reasoning_tests.rs"]
mod run_progress_reasoning_tests;
#[cfg(test)]
#[path = "run_progress_tests.rs"]
mod run_progress_tests;
#[cfg(test)]
#[path = "run_progress_text_tool_call_tests.rs"]
mod run_progress_text_tool_call_tests;
