//! Provider turn 循环内 Token Budget 窗口轮转。
//!
//! 根据 provider usage 执行零模型窗口轮转，并重建 replacement history。
//! System/Developer Prompt 前缀保持不变；轮转事实由本地 SessionNotes 提供。

use axon_display::{format_decimal_count, format_duration_millis_compact};
use axon_domain::{AxonError, AxonResult, TurnId};

use crate::agent_model_tool_history::normalize_provider_history;
use crate::insert_then_compress_classification::compressible_len;
use crate::token_budget_guard::{StepPace, TokenBudgetRuntimePolicy};
pub use crate::token_estimator::{
    estimated_history_tokens, estimated_history_tokens_for_provider,
    estimated_history_tokens_for_provider_with_capabilities,
};
use crate::{
    AgentModelHistorySnapshot, AgentModelMessage, AgentRuntimeNoteKind, AgentTurnEvent,
    AgentTurnEventKind, CanonicalCheckpoint, ModelToolDefinition, PromptCacheProviderWireFormat,
};

#[path = "insert_then_compress_history.rs"]
mod history;
pub use history::CompactedHistory;
#[path = "insert_then_compress_completion.rs"]
mod completion;
#[path = "insert_then_compress_digest.rs"]
mod digest;
#[path = "budget_notice.rs"]
mod notice;
use completion::{
    CompactionCompletionContext, CompactionFailureMetrics, complete_compaction,
    compression_failed_event,
};
#[cfg(test)]
pub(crate) use digest::rebuild_with_digest_for_test;
use history::{FreshWindowContext, build_fresh_window_history_from_normalized};

const ROLLOVER_FAILURE_NO_PROGRESS: &str = "no_progress";
const ROLLOVER_TRIGGER_REQUESTED: &str = "requested";
const ROLLOVER_TRIGGER_AUTOMATIC: &str = "automatic";

/// 窗口轮转 checkpoint 前缀，向 agent 标识当前任务交接事实。
pub const WINDOW_ROLLOVER_CHECKPOINT_PREFIX: &str = "当前窗口交接事实：";

/// 自动换窗连续失败后的退避步长（token）。
///
/// 失败原因（便签超预算、消息数未收敛、投影写失败）通常在同一上下文读数上持续存在，
/// 逐轮重试只会让每一轮都付一次「归档加回滚」的整份语料成本。这里让每次连续失败都
/// 要求上下文再涨一个步长才允许重试：读数单调增长，最终一定会越过退避线，因此不会
/// 永久放弃自救；而同一读数上的重复尝试被压掉。
const AUTO_ROLLOVER_RETRY_BACKOFF_TOKENS: u64 = 4_096;

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
}

impl ProviderVisibleWindow {
    /// 返回参与 checkpoint、校验与归档的同一消息序列。
    #[must_use]
    pub fn messages(&self) -> &[AgentModelMessage] {
        &self.messages
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
#[derive(Clone)]
pub struct WindowRolloverPolicyRuntime {
    /// 压缩策略。
    pub policy: CompressionPolicy,
    fresh_window_context: Option<FreshWindowContext>,
    supports_vision: bool,
    token_budget: TokenBudgetRuntimePolicy,
    /// 当前窗口是否已发过收尾预警。
    warn_emitted: bool,
    /// 当前窗口是否已发过中段保值提示。
    midway_emitted: bool,
    fallback_emitted: bool,
    /// 模型单步上下文消耗的滚动采样。
    step_pace: StepPace,
    /// 距上一次便签写入的 assistant 步数。
    steps_since_notes: usize,
    /// 连续失败的自动换窗次数。
    auto_rollover_failure_streak: u32,
    /// 上一次自动换窗失败时的上下文读数，用于退避比较。
    last_auto_rollover_failure_tokens: Option<u64>,
    /// 最近动作摘要的来源；由调用方在构造时注入（它才拿得到本轮的工具回执）。
    digest_source: Option<std::rc::Rc<dyn Fn() -> Option<String>>>,
    /// 下一次换窗要注入的最近动作摘要。
    rollover_digest: Option<String>,
}

impl WindowRolloverPolicyRuntime {
    /// 创建新实例。
    #[must_use]
    pub fn new(policy: CompressionPolicy) -> Self {
        Self {
            policy,
            fresh_window_context: None,
            supports_vision: true,
            token_budget: TokenBudgetRuntimePolicy::default(),
            warn_emitted: false,
            midway_emitted: false,
            fallback_emitted: false,
            step_pace: StepPace::default(),
            steps_since_notes: 0,
            auto_rollover_failure_streak: 0,
            last_auto_rollover_failure_tokens: None,
            digest_source: None,
            rollover_digest: None,
        }
    }

    /// 注入「最近动作摘要」的来源。
    ///
    /// 摘要必须来自本轮真实的工具回执：`turn_events` 里只有「记录了几条回执」这类
    /// 计数事件，拿不到工具名与状态。调用方持有回执，因此由它提供闭包，换窗时现取。
    #[must_use]
    pub fn with_digest_source(mut self, source: std::rc::Rc<dyn Fn() -> Option<String>>) -> Self {
        self.digest_source = Some(source);
        self
    }

    /// 取一次最近动作摘要；没有来源时返回 `None`。
    fn take_digest(&self) -> Option<String> {
        self.digest_source
            .as_ref()
            .and_then(|source| source())
            .filter(|digest| !digest.trim().is_empty())
    }
    /// 绑定当前 turn 的 provider 可见投影配置。
    ///
    /// 工具定义与 wire format 已不再参与轮转计算：换窗只做历史重建，不再产出任何
    /// token 读数。签名保留是为了不扩散调用面，参数被显式忽略。
    #[must_use]
    pub fn with_provider_wire(
        self,
        _format: PromptCacheProviderWireFormat,
        _model_tools: Vec<ModelToolDefinition>,
    ) -> Self {
        self
    }
    /// 绑定当前 turn 的模型视觉能力（其余配置参数已不参与轮转计算）。
    #[must_use]
    pub fn with_provider_wire_capabilities(
        mut self,
        _format: PromptCacheProviderWireFormat,
        _model_tools: Vec<ModelToolDefinition>,
        supports_vision: bool,
    ) -> Self {
        self.supports_vision = supports_vision;
        self
    }
    pub(crate) fn configure_provider_wire(
        &mut self,
        _format: PromptCacheProviderWireFormat,
        _model_tools: &[ModelToolDefinition],
        supports_vision: bool,
    ) {
        self.supports_vision = supports_vision;
    }

    /// 构造本次轮转唯一的规范化 Provider 可见视图。
    #[must_use]
    pub fn provider_visible_window(&self, messages: &[AgentModelMessage]) -> ProviderVisibleWindow {
        let normalized = normalize_provider_history(messages, self.supports_vision);
        ProviderVisibleWindow {
            messages: normalized.messages.into_owned(),
        }
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
    pub(crate) fn record_outcome(&mut self, outcome: CompressionOutcome) {
        match outcome {
            CompressionOutcome::Compressed => {
                self.warn_emitted = false;
                self.midway_emitted = false;
                self.fallback_emitted = false;
                // 新窗口的步幅与旧窗口无关，采样必须从零开始。
                self.step_pace.reset();
                self.steps_since_notes = 0;
                // 换窗成功即解除退避：新窗口的失败计数与旧窗口无关。
                self.auto_rollover_failure_streak = 0;
                self.last_auto_rollover_failure_tokens = None;
            }
            CompressionOutcome::Prompted | CompressionOutcome::Idle => {}
        }
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
                && self.auto_rollover_backoff_satisfied(tokens)
        })
    }

    /// 记一次自动换窗失败，并把重试门槛抬到「本次读数加一个退避步长」。
    ///
    /// 只由自动路径调用：模型主动换窗是需要它自己看回执后重试的决策，不该被运行时代为退避。
    pub(crate) fn note_auto_rollover_failure(&mut self, current_context_tokens: u64) {
        self.auto_rollover_failure_streak = self.auto_rollover_failure_streak.saturating_add(1);
        self.last_auto_rollover_failure_tokens = Some(current_context_tokens);
    }

    /// 上下文是否已经涨过上一次自动换窗失败的重试门槛。
    ///
    /// 门槛随连续失败次数线性抬高，并夹在模型窗口内：读数单调增长，因此最终一定会越过，
    /// 不会把会话永久钉在满上下文上；夹断只避免要求值超出窗口本身。
    fn auto_rollover_backoff_satisfied(&self, current_context_tokens: u64) -> bool {
        let Some(last) = self.last_auto_rollover_failure_tokens else {
            return true;
        };
        let streak = u64::from(self.auto_rollover_failure_streak.max(1));
        let required = last
            .saturating_add(AUTO_ROLLOVER_RETRY_BACKOFF_TOKENS.saturating_mul(streak))
            .min(self.policy.context_window);
        current_context_tokens >= required
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
            notice::fallback_note(),
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
            self.rollover_digest.as_deref(),
        )
    }
}

/// replacement history 的校验强度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowRolloverValidationMode {
    /// Agent 或用户主动开启新窗口：只校交接摘要预算，窗口体量交给 Provider 实测。
    Requested,
    /// 系统自动开启新窗口：额外要求相对旧窗口取得消息数收敛。
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

    /// 记录本次失败的**具体原因**，供随后的 [`Self::abort`] 投影使用。
    ///
    /// 默认实现忽略：只有真正持有诊断状态的事务才需要覆盖它。有了这条约定，
    /// `abort` 就不必再用「未通过门禁」这类默认文案盖住真实错误。
    /// 换窗是会话自救的唯一动作，失败原因不可辨等于把会话钉死在满上下文。
    fn note_failure_reason(&mut self, _reason: &str) {}

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
}

impl WindowRolloverRuntime<'_> {
    /// 创建循环内压缩适配器。
    #[must_use]
    pub fn new(
        runtime: WindowRolloverPolicyRuntime,
        rollover: &mut dyn WindowRolloverTransaction,
    ) -> WindowRolloverRuntime<'_> {
        WindowRolloverRuntime { runtime, rollover }
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
        if let Some(current) = current_context_tokens {
            self.runtime.observe_context(current);
        }
        if !force_rollover
            && self
                .runtime
                .maybe_emit_budget_notice(messages, current_context_tokens)
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
        // 起始摘要只报「正在开启」：换窗前的水位属于 before 读数，已整体下线。
        let start_summary = if force_rollover {
            "正在按当前换窗请求开启新的记忆窗口 · 已耗时 0ms".to_owned()
        } else {
            "正在开启新的记忆窗口 · 已耗时 0ms".to_owned()
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
            })),
        );
        let duration_ms = || now_ms().saturating_sub(started_at_ms);
        // 便签由模型自己写，常常只剩一句结论；最近两步工具动作由调用方的来源补进新窗口。
        self.runtime.rollover_digest = self.runtime.take_digest();
        let outcome = match self.rollover.prepare(provider_messages) {
            Ok(checkpoint) => complete_compaction(
                CompactionCompletionContext {
                    runtime: &self.runtime,
                    checkpoint: &checkpoint,
                    turn_id,
                    duration_ms: duration_ms(),
                    provider_messages,
                    validation_mode,
                },
                messages,
                turn_events,
                self.rollover,
            ),
            Err(error) => {
                // 事务实现自己也会记录原因；这里再写一次幂等覆盖，保证任何实现
                // （含只返回 Err 的闭包适配器）都不会让 abort 落到默认文案上。
                self.rollover
                    .note_failure_reason(&format!("本地窗口轮转失败：{error:?}"));
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
            // 失败一次就把自动重试门槛抬高：同一读数上再试一次只会重复付归档与回滚，
            // 而失败原因（便签超预算、消息数未收敛、投影写失败）往往要等上下文再涨或
            // 外部条件变化才可能好转。
            if validation_mode == WindowRolloverValidationMode::Automatic {
                self.runtime
                    .note_auto_rollover_failure(current_context_tokens.unwrap_or_default());
            }
        }
        self.runtime.record_outcome(outcome);
        outcome
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
#[path = "budget_notice_tests.rs"]
mod budget_notice_tests;
