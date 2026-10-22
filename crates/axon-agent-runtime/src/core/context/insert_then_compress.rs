//! Provider turn 循环内 Token Budget 窗口轮转。
//!
//! 根据 provider usage 执行零模型窗口轮转，并重建 replacement history。
//! System/Developer Prompt 前缀保持不变；轮转事实由本地 SessionNotes 提供。

use axon_crypto::sha256_hex;
use axon_display::{format_decimal_count, format_duration_millis_compact};
use axon_domain::{AxonError, AxonResult, TurnId};

use crate::agent_model_tool_history::normalize_provider_history;
use crate::insert_then_compress_classification::compressible_len;
use crate::token_budget_guard::TokenBudgetRuntimePolicy;
pub use crate::token_estimator::{
    estimated_history_tokens, estimated_history_tokens_for_provider,
    estimated_history_tokens_for_provider_with_capabilities,
};
use crate::{
    AgentModelHistorySnapshot, AgentModelMessage, AgentModelMessageKind, AgentRuntimeNoteKind,
    AgentTurnEvent, AgentTurnEventKind, CanonicalCheckpoint, ModelToolDefinition,
    PromptCacheProviderWireFormat, ProviderVisibleProjectionMetrics,
};

#[path = "insert_then_compress_history.rs"]
mod history;
pub use history::CompactedHistory;
#[path = "insert_then_compress_completion.rs"]
mod completion;
use completion::{
    CompactionCompletionContext, CompactionFailureMetrics, complete_compaction,
    compression_failed_event,
};
use history::{FreshWindowContext, build_fresh_window_history_from_normalized};

const ROLLOVER_FAILURE_CONTEXT_OVERFLOW: &str = "context_overflow";
const ROLLOVER_FAILURE_NO_PROGRESS: &str = "no_progress";
const ROLLOVER_TRIGGER_REQUESTED: &str = "requested";
const ROLLOVER_TRIGGER_AUTOMATIC: &str = "automatic";

const TOKEN_BUDGET_REMINDER_MESSAGE_PREFIX: &str = "当前任务窗口距离自动换窗还剩 ";
const TOKEN_BUDGET_FALLBACK_MESSAGE: &str = "当前任务窗口已耗尽。不要在本窗口继续任务或给出最终答复，新窗口不会自动包含本次对话。现在只允许一次便签写入或追加：调用 builtin.notes_write_file 或 builtin.notes_append_to_file 保存简洁 checkpoint，包含目标、关键决策、进展、经验教训、下一步动作，以及仍在处理的相关用户请求和重要工具动作的 window ID 与 item ID；每条非 assistant 条目正文末尾都带有 `[id: ...]` 标记，可原样引用。便签返回后立即调用 builtin.new_context，不要使用除便签与 new_context 之外的任何工具。";

/// 窗口轮转 checkpoint 前缀，向 agent 标识当前任务交接事实。
pub const WINDOW_ROLLOVER_CHECKPOINT_PREFIX: &str = "当前窗口交接事实：";

/// 循环内窗口轮转策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressionPolicy {
    /// 触发窗口轮转的 provider 当前上下文 token 阈值。
    pub trigger_tokens: u64,
    /// Provider 声明的上下文窗口。
    pub context_window: u64,
}

/// 单次窗口轮转使用的规范化 Provider 可见视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderVisibleWindow {
    messages: Vec<AgentModelMessage>,
    tokens: u64,
}

impl ProviderVisibleWindow {
    /// 返回参与 checkpoint、校验与归档的同一消息序列。
    #[must_use]
    pub fn messages(&self) -> &[AgentModelMessage] {
        &self.messages
    }

    /// 返回该消息序列的本地 token 估算。
    #[must_use]
    pub const fn tokens(&self) -> u64 {
        self.tokens
    }
}

impl CompressionPolicy {
    /// 按模型上下文窗口与阈值比例推导策略。
    ///
    /// `context_window == 0` 表示模型未声明窗口，返回 `Ok(None)`（禁用压缩）；
    /// 阈值超出 `0.0..=1.0` 属于配置错误。
    pub fn for_context_window(context_window: u64, threshold: f64) -> AxonResult<Option<Self>> {
        if !(0.0..=1.0).contains(&threshold) {
            return Err(AxonError::runtime_failure(format!(
                "自动压缩阈值必须在 0.0-1.0 之间，当前值：{threshold}"
            )));
        }
        if context_window == 0 {
            return Ok(None);
        }
        // context_window 不会超过 f64 精度范围（2^53），精度损失可接受。
        let trigger_tokens = ((context_window as f64 * threshold).round() as u64).max(1);
        Ok(Some(Self {
            trigger_tokens,
            context_window,
        }))
    }
}

/// Token Budget 窗口轮转策略运行时。
#[derive(Debug, Clone)]
pub struct WindowRolloverPolicyRuntime {
    /// 压缩策略。
    pub policy: CompressionPolicy,
    fresh_window_context: Option<FreshWindowContext>,
    provider_format: PromptCacheProviderWireFormat,
    model_tools: Vec<ModelToolDefinition>,
    supports_vision: bool,
    token_budget: TokenBudgetRuntimePolicy,
    reminder_emitted: bool,
    fallback_emitted: bool,
}

impl WindowRolloverPolicyRuntime {
    /// 创建新实例。
    #[must_use]
    pub fn new(policy: CompressionPolicy) -> Self {
        Self {
            policy,
            fresh_window_context: None,
            provider_format: PromptCacheProviderWireFormat::OpenAiCompatibleChat,
            model_tools: Vec::new(),
            supports_vision: true,
            token_budget: TokenBudgetRuntimePolicy::default(),
            reminder_emitted: false,
            fallback_emitted: false,
        }
    }
    /// 绑定当前 turn 的 provider 可见投影配置。
    #[must_use]
    pub fn with_provider_wire(
        self,
        format: PromptCacheProviderWireFormat,
        model_tools: Vec<ModelToolDefinition>,
    ) -> Self {
        self.with_provider_wire_capabilities(format, model_tools, true)
    }
    /// 绑定当前 turn 的 provider 可见投影配置和模型视觉能力。
    #[must_use]
    pub fn with_provider_wire_capabilities(
        mut self,
        format: PromptCacheProviderWireFormat,
        model_tools: Vec<ModelToolDefinition>,
        supports_vision: bool,
    ) -> Self {
        self.provider_format = format;
        self.model_tools = model_tools;
        self.supports_vision = supports_vision;
        self
    }
    pub(crate) fn configure_provider_wire(
        &mut self,
        format: PromptCacheProviderWireFormat,
        model_tools: &[ModelToolDefinition],
        supports_vision: bool,
    ) {
        self.provider_format = format;
        self.model_tools.clear();
        self.model_tools.extend_from_slice(model_tools);
        self.supports_vision = supports_vision;
    }

    /// 构造本次轮转唯一的规范化 Provider 可见视图。
    #[must_use]
    pub fn provider_visible_window(&self, messages: &[AgentModelMessage]) -> ProviderVisibleWindow {
        let normalized = normalize_provider_history(messages, self.supports_vision);
        let messages = normalized.messages.into_owned();
        let tokens = estimated_history_tokens_for_provider_with_capabilities(
            messages.as_slice(),
            self.provider_format,
            &self.model_tools,
            self.supports_vision,
        );
        ProviderVisibleWindow { messages, tokens }
    }
    /// 从当前运行态消息恢复窗口内提醒去重标记。
    ///
    /// 只扫描运行态标记，不做历史规范化或深拷贝；FreshWindow 装配上下文推迟到
    /// 门禁通过的换窗时刻捕获，避免长会话在每轮 turn 顶部重复复制完整历史。
    pub(crate) fn refresh_rollover_markers(&mut self, messages: &[AgentModelMessage]) {
        let fallback_note = crate::runtime_note_content(
            AgentRuntimeNoteKind::PayloadBudgetGate,
            TOKEN_BUDGET_FALLBACK_MESSAGE,
        );
        let mut reminder_emitted = false;
        let mut fallback_emitted = false;
        for message in messages {
            if !matches!(
                &message.kind,
                AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::PayloadBudgetGate)
            ) {
                continue;
            }
            if message
                .content
                .contains(TOKEN_BUDGET_REMINDER_MESSAGE_PREFIX)
            {
                reminder_emitted = true;
            }
            if message.content == fallback_note {
                reminder_emitted = true;
                fallback_emitted = true;
            }
        }
        self.reminder_emitted = reminder_emitted;
        self.fallback_emitted = fallback_emitted;
    }

    /// 用本次轮转唯一的规范化历史捕获 FreshWindow 装配上下文。
    ///
    /// 必须在 `provider_visible_window` 与保存代际门禁之后调用，确保捕获的稳定
    /// 前缀与归档历史同源，且失败路径不产生整段历史的深拷贝。
    fn capture_fresh_window_context(&mut self, provider_messages: &[AgentModelMessage]) {
        self.fresh_window_context = Some(FreshWindowContext::capture_from_normalized(
            provider_messages,
        ));
    }

    /// 仅供测试：一次性刷新标记并捕获 FreshWindow 上下文。
    #[cfg(test)]
    pub(crate) fn refresh_fresh_window_context(&mut self, messages: &[AgentModelMessage]) {
        self.refresh_rollover_markers(messages);
        self.fresh_window_context =
            Some(FreshWindowContext::capture(messages, self.supports_vision));
    }
    fn projection_history_fingerprint_from_normalized(
        &self,
        messages: &[AgentModelMessage],
    ) -> Option<(String, usize)> {
        serde_json::to_vec(messages)
            .ok()
            .map(|bytes| (sha256_hex(&bytes), messages.len()))
    }
    fn record_outcome(&mut self, outcome: CompressionOutcome) {
        match outcome {
            CompressionOutcome::Compressed => {
                self.reminder_emitted = false;
                self.fallback_emitted = false;
            }
            CompressionOutcome::Prompted | CompressionOutcome::Idle => {}
        }
    }
    fn maybe_emit_budget_reminder(
        &mut self,
        messages: &mut AgentModelHistorySnapshot,
        current_context_tokens: Option<u64>,
    ) -> bool {
        let Some(current) = current_context_tokens else {
            return false;
        };
        if current >= self.policy.trigger_tokens {
            return false;
        }
        let remaining = TokenBudgetRuntimePolicy::remaining_before_rollover(
            self.policy.trigger_tokens,
            current,
        );
        if self.reminder_emitted
            || !self
                .token_budget
                .should_emit_reminder(self.policy.trigger_tokens, current)
        {
            return false;
        }
        messages.push(AgentModelMessage::runtime_note(
            AgentRuntimeNoteKind::PayloadBudgetGate,
            format!(
                "{TOKEN_BUDGET_REMINDER_MESSAGE_PREFIX}{remaining} tokens。请在当前阶段结束前调用 builtin.notes_write_file 或 builtin.notes_append_to_file 写入任务便签（目标、关键决策、进展、经验教训、下一步动作，以及相关条目的 window ID 与 item ID）；写入后可调用 builtin.new_context 主动开启新窗口。旧消息会保存在会话记录中，可在新窗口用 builtin.history_list_items 或 builtin.history_search_contents 按需查找。"
            ),
        ));
        self.reminder_emitted = true;
        true
    }
    /// 根据 Provider usage 与 fallback 缓冲判断是否必须轮转窗口。
    #[must_use]
    pub fn should_compress(
        &self,
        messages: &[AgentModelMessage],
        current_context_tokens: Option<u64>,
    ) -> bool {
        if compressible_len(messages) < 2 {
            return false;
        }
        current_context_tokens.is_some_and(|tokens| {
            tokens > 0
                && tokens
                    >= self.token_budget.forced_rollover_tokens(
                        self.policy.trigger_tokens,
                        self.policy.context_window,
                    )
        })
    }

    fn should_emit_fallback(&self, current_context_tokens: u64) -> bool {
        self.token_budget.should_emit_fallback(
            self.policy.trigger_tokens,
            self.policy.context_window,
            current_context_tokens,
            self.fallback_emitted,
        )
    }

    pub(crate) fn emit_fallback(&mut self, messages: &mut AgentModelHistorySnapshot) {
        if self.fallback_emitted {
            return;
        }
        self.fallback_emitted = true;
        messages.push(AgentModelMessage::runtime_note(
            AgentRuntimeNoteKind::PayloadBudgetGate,
            TOKEN_BUDGET_FALLBACK_MESSAGE,
        ));
    }

    /// 仅供测试直接检查 FreshWindow 结构；生产路径必须调用 build_validated_replacement。
    #[cfg(test)]
    #[must_use]
    pub fn rebuild_with_compression(
        &self,
        messages: &[AgentModelMessage],
        checkpoint: &CanonicalCheckpoint,
    ) -> CompactedHistory {
        let normalized = normalize_provider_history(messages, self.supports_vision);
        self.rebuild_with_compression_from_normalized(normalized.messages.as_ref(), checkpoint)
    }

    fn rebuild_with_compression_from_normalized(
        &self,
        provider_messages: &[AgentModelMessage],
        checkpoint: &CanonicalCheckpoint,
    ) -> CompactedHistory {
        build_fresh_window_history_from_normalized(
            provider_messages,
            checkpoint,
            self.fresh_window_context.as_ref(),
            self.provider_format,
            &self.model_tools,
            self.supports_vision,
        )
    }
}

/// replacement history 的校验强度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowRolloverValidationMode {
    /// Agent 或用户主动开启新窗口，只要求新窗口处于安全边界内。
    Requested,
    /// 系统自动开启新窗口，还要求相对旧窗口取得量化收敛。
    Automatic,
}

impl WindowRolloverValidationMode {
    const fn requires_progress(self) -> bool {
        matches!(self, Self::Automatic)
    }

    const fn trigger_code(self) -> &'static str {
        match self {
            Self::Requested => ROLLOVER_TRIGGER_REQUESTED,
            Self::Automatic => ROLLOVER_TRIGGER_AUTOMATIC,
        }
    }
}

/// turn loop 循环内压缩的单次结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompressionOutcome {
    /// 未达到阈值，无需压缩。
    Idle,
    /// 压缩完成，历史已重建。
    Compressed,
    /// 轮转失败，保留可安全发送的历史并附加修复提示，继续当前 invocation。
    Prompted,
}
/// 窗口轮转持久化事务。
///
/// `prepare` 只能生成 checkpoint，不得归档历史或推进窗口；`commit` 仅在
/// replacement history 完整校验通过后调用。
pub trait WindowRolloverTransaction {
    /// 生成待校验的窗口交接 checkpoint。
    fn prepare(&mut self, active_messages: &[AgentModelMessage])
    -> AxonResult<CanonicalCheckpoint>;

    /// 自动换窗前校验记忆交接；主动换窗同样必须先完成当前窗口保存。
    fn validate_memory_handoff(
        &mut self,
        _validation_mode: WindowRolloverValidationMode,
    ) -> AxonResult<()> {
        Ok(())
    }

    /// 当前记忆窗口 ID；用于给 provider 可见历史渲染稳定 item id 标记。
    fn current_window_id(&mut self) -> Option<String> {
        None
    }

    /// 发布已经通过校验的窗口归档与代际状态。
    fn commit(
        &mut self,
        active_messages: &[AgentModelMessage],
        checkpoint: &CanonicalCheckpoint,
        replacement: &CompactedHistory,
    ) -> AxonResult<()>;

    /// replacement history 已安装后的无失败完成通知。
    fn complete(&mut self) {}

    /// 发布成功换窗后的活动历史，供后续请求与进程重启恢复。
    fn publish_active_window(&mut self, _messages: &[AgentModelMessage]) -> AxonResult<()> {
        Ok(())
    }

    /// prepare、校验或 commit 失败后的状态清理通知。
    fn abort(&mut self) {}
}

impl<F> WindowRolloverTransaction for F
where
    F: FnMut(&[AgentModelMessage]) -> AxonResult<CanonicalCheckpoint>,
{
    fn prepare(
        &mut self,
        active_messages: &[AgentModelMessage],
    ) -> AxonResult<CanonicalCheckpoint> {
        self(active_messages)
    }

    fn commit(
        &mut self,
        _active_messages: &[AgentModelMessage],
        _checkpoint: &CanonicalCheckpoint,
        _replacement: &CompactedHistory,
    ) -> AxonResult<()> {
        Ok(())
    }
}
/// turn loop 循环内压缩适配器。
pub struct WindowRolloverRuntime<'a> {
    /// 压缩引擎与策略。
    pub runtime: WindowRolloverPolicyRuntime,
    /// 先准备 checkpoint，校验 replacement 后再提交归档。
    pub rollover: &'a mut dyn WindowRolloverTransaction,
    pub(crate) last_projection_metrics: Option<ProviderVisibleProjectionMetrics>,
}

impl WindowRolloverRuntime<'_> {
    /// 创建循环内压缩适配器。
    #[must_use]
    pub fn new(
        runtime: WindowRolloverPolicyRuntime,
        rollover: &mut dyn WindowRolloverTransaction,
    ) -> WindowRolloverRuntime<'_> {
        WindowRolloverRuntime {
            runtime,
            rollover,
            last_projection_metrics: None,
        }
    }
    /// 若 Provider usage 达到阈值则轮转窗口，并记录生命周期。
    pub(crate) fn maybe_compress(
        &mut self,
        messages: &mut AgentModelHistorySnapshot,
        current_context_tokens: Option<u64>,
        turn_id: TurnId,
        turn_events: &mut Vec<AgentTurnEvent>,
    ) -> CompressionOutcome {
        self.maybe_compress_inner(
            messages,
            current_context_tokens,
            false,
            turn_id,
            turn_events,
        )
    }

    /// 处理模型通过 `new_context` 发起的主动换窗请求，跳过 fallback 缓冲。
    pub(crate) fn maybe_compress_requested(
        &mut self,
        messages: &mut AgentModelHistorySnapshot,
        current_context_tokens: Option<u64>,
        turn_id: TurnId,
        turn_events: &mut Vec<AgentTurnEvent>,
    ) -> CompressionOutcome {
        self.maybe_compress_inner(messages, current_context_tokens, true, turn_id, turn_events)
    }

    fn maybe_compress_inner(
        &mut self,
        messages: &mut AgentModelHistorySnapshot,
        current_context_tokens: Option<u64>,
        force_rollover: bool,
        turn_id: TurnId,
        turn_events: &mut Vec<AgentTurnEvent>,
    ) -> CompressionOutcome {
        self.last_projection_metrics = None;
        if !force_rollover
            && self
                .runtime
                .maybe_emit_budget_reminder(messages, current_context_tokens)
        {
            return CompressionOutcome::Idle;
        }
        if !force_rollover
            && let Some(current) = current_context_tokens
            && self.runtime.should_emit_fallback(current)
        {
            self.runtime.emit_fallback(messages);
            return CompressionOutcome::Idle;
        }
        let should_compress = force_rollover
            || self
                .runtime
                .should_compress(messages.as_slice(), current_context_tokens);
        if !should_compress {
            return CompressionOutcome::Idle;
        }
        let before_tokens = current_context_tokens.unwrap_or_default();
        let validation_mode = if force_rollover {
            WindowRolloverValidationMode::Requested
        } else {
            WindowRolloverValidationMode::Automatic
        };
        if let Err(error) = self.rollover.validate_memory_handoff(validation_mode) {
            self.rollover.abort();
            turn_events.push(compression_failed_event(
                turn_id,
                &format!("换窗前任务状态未完成保存：{error:?}"),
                0,
                "memory_handoff_missing",
                CompactionFailureMetrics {
                    trigger: validation_mode.trigger_code(),
                    before_tokens: current_context_tokens.unwrap_or_default(),
                    after_tokens: 0,
                    before_messages: messages.len(),
                    after_messages: messages.len(),
                },
            ));
            // 未保存时保留历史和修复提示，但不得继续发送原窗口。
            self.runtime.emit_fallback(messages);
            return CompressionOutcome::Prompted;
        }
        // 门禁通过后才捕获 FreshWindow 装配上下文，失败路径不复制历史。
        let provider_window = self.runtime.provider_visible_window(messages.as_slice());
        let provider_messages = provider_window.messages();
        self.runtime.capture_fresh_window_context(provider_messages);
        let started_at_ms = now_ms();
        let start_summary = if force_rollover {
            "正在按当前换窗请求开启新的记忆窗口 · 已耗时 0ms".to_owned()
        } else {
            format!(
                "正在开启新的记忆窗口 · 已耗时 0ms：当前上下文 {} tokens ≥ 强制换窗线 {}",
                format_decimal_count(before_tokens),
                format_decimal_count(self.runtime.token_budget.forced_rollover_tokens(
                    self.runtime.policy.trigger_tokens,
                    self.runtime.policy.context_window,
                ))
            )
        };
        turn_events.push(
            AgentTurnEvent::new(
                turn_id,
                AgentTurnEventKind::CompactionStarted,
                start_summary,
            )
            .with_phase("history")
            .with_status("running")
            .with_metadata(serde_json::json!({
                "trigger": validation_mode.trigger_code(),
                "beforeTokens": before_tokens,
            })),
        );
        let duration_ms = || now_ms().saturating_sub(started_at_ms);
        let source_tokens = provider_window.tokens();
        let (projection_input_hash, source_message_count) = self
            .runtime
            .projection_history_fingerprint_from_normalized(provider_messages)
            .unwrap_or_else(|| (String::new(), provider_messages.len()));
        let mut result_tokens = None;
        let outcome = match self.rollover.prepare(provider_messages) {
            Ok(checkpoint) => complete_compaction(
                CompactionCompletionContext {
                    runtime: &self.runtime,
                    checkpoint: &checkpoint,
                    before_tokens,
                    source_tokens,
                    turn_id,
                    duration_ms: duration_ms(),
                    provider_messages,
                    validation_mode,
                },
                messages,
                turn_events,
                &mut result_tokens,
                self.rollover,
            ),
            Err(error) => {
                self.rollover.abort();
                turn_events.push(compression_failed_event(
                    turn_id,
                    &format!(
                        "本地窗口轮转失败；模型可见历史变化后，下一个安全 turn 将重试：{error:?}"
                    ),
                    duration_ms(),
                    "request_failed",
                    CompactionFailureMetrics {
                        trigger: validation_mode.trigger_code(),
                        before_tokens,
                        after_tokens: 0,
                        before_messages: messages.len(),
                        after_messages: messages.len(),
                    },
                ));
                // 保留当前历史和修复提示，允许当前 invocation 继续完成。
                CompressionOutcome::Prompted
            }
        };
        if outcome == CompressionOutcome::Prompted {
            self.runtime.emit_fallback(messages);
        }
        if outcome == CompressionOutcome::Compressed {
            let result_tokens = result_tokens.unwrap_or_default();
            let (projection_output_hash, _) = self
                .runtime
                .projection_history_fingerprint_from_normalized(messages.as_slice())
                .unwrap_or_else(|| (String::new(), messages.len()));
            self.last_projection_metrics = Some(ProviderVisibleProjectionMetrics {
                source_tokens,
                result_tokens,
                saved_tokens: source_tokens.saturating_sub(result_tokens),
                covered_message_count: source_message_count,
                projection_input_hash,
                projection_output_hash,
                transcript_generation: turn_id.0,
                projection_generation: turn_id.0.saturating_add(1),
            });
        }
        self.runtime.record_outcome(outcome);
        outcome
    }
    /// 取出最近一次成功压缩的 projection 计量。
    pub(crate) fn take_projection_metrics(&mut self) -> Option<ProviderVisibleProjectionMetrics> {
        self.last_projection_metrics.take()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
