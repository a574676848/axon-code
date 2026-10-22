use super::*;
use crate::session_notes::SESSION_NOTES_CHECKPOINT_MAX_BYTES;

pub(super) struct CompactionCompletionContext<'a> {
    pub(super) runtime: &'a WindowRolloverPolicyRuntime,
    pub(super) checkpoint: &'a CanonicalCheckpoint,
    pub(super) before_tokens: u64,
    pub(super) source_tokens: u64,
    pub(super) turn_id: TurnId,
    pub(super) duration_ms: u64,
    pub(super) provider_messages: &'a [AgentModelMessage],
    pub(super) validation_mode: WindowRolloverValidationMode,
}

pub(super) struct CompactionFailureMetrics {
    pub(super) trigger: &'static str,
    pub(super) before_tokens: u64,
    pub(super) after_tokens: u64,
    pub(super) before_messages: usize,
    pub(super) after_messages: usize,
}

pub(super) struct CompactionCompletedMetrics {
    pub(super) trigger: &'static str,
    pub(super) before_tokens: u64,
    pub(super) after_tokens: u64,
    pub(super) before_messages: usize,
    pub(super) after_messages: usize,
    pub(super) checkpoint_bytes: usize,
}

impl WindowRolloverPolicyRuntime {
    /// 构建并完整校验可发布的 FreshWindow replacement history。
    pub fn build_validated_replacement(
        &self,
        window: &ProviderVisibleWindow,
        checkpoint: &CanonicalCheckpoint,
        validation_mode: WindowRolloverValidationMode,
    ) -> AxonResult<CompactedHistory> {
        let provider_messages = window.messages();
        let source_tokens = window.tokens();
        validate_replacement(
            self,
            checkpoint,
            provider_messages,
            source_tokens,
            provider_messages.len(),
            validation_mode,
        )
        .map(|validated| validated.compacted)
        .map_err(|error| {
            let (summary, _) = validation_failure_details(
                self,
                error.compacted(),
                source_tokens,
                provider_messages.len(),
                &error,
            );
            AxonError::runtime_failure(summary)
        })
    }
}

pub(super) fn complete_compaction(
    context: CompactionCompletionContext<'_>,
    messages: &mut AgentModelHistorySnapshot,
    turn_events: &mut Vec<AgentTurnEvent>,
    result_tokens: &mut Option<u64>,
    transaction: &mut dyn WindowRolloverTransaction,
) -> CompressionOutcome {
    let validated = match validate_replacement(
        context.runtime,
        context.checkpoint,
        context.provider_messages,
        context.source_tokens,
        context.provider_messages.len(),
        context.validation_mode,
    ) {
        Ok(validated) => validated,
        Err(error) => {
            transaction.abort();
            turn_events.push(validation_failed_event(
                context.runtime,
                context.source_tokens,
                context.provider_messages.len(),
                context.turn_id,
                context.duration_ms,
                context.validation_mode,
                error,
            ));
            return CompressionOutcome::Prompted;
        }
    };
    *result_tokens = Some(validated.compacted.after_tokens);
    if let Err(error) = transaction.commit(
        context.provider_messages,
        context.checkpoint,
        &validated.compacted,
    ) {
        transaction.abort();
        turn_events.push(commit_failed_event(
            context.turn_id,
            context.duration_ms,
            context.source_tokens,
            validated.compacted.after_tokens,
            validated.before_messages,
            validated.compacted.messages.len(),
            context.validation_mode,
            &error,
        ));
        // durable 投影失败时继续当前 agent turn，但只发送已校验的 FreshWindow
        // replacement history，避免把旧工具输出重新带回 Provider。
        *messages = validated.compacted.messages.into();
        return CompressionOutcome::Prompted;
    }

    let after_messages = validated.compacted.messages.len();
    *messages = validated.compacted.messages.into();
    turn_events.push(compaction_completed_event(
        context.turn_id,
        context.duration_ms,
        CompactionCompletedMetrics {
            trigger: context.validation_mode.trigger_code(),
            before_tokens: context.before_tokens,
            after_tokens: validated.compacted.after_tokens,
            before_messages: validated.before_messages,
            after_messages,
            checkpoint_bytes: validated.checkpoint_bytes,
        },
    ));
    transaction.complete();
    CompressionOutcome::Compressed
}

struct ValidatedCompaction {
    compacted: CompactedHistory,
    before_messages: usize,
    checkpoint_bytes: usize,
}

enum ReplacementValidationError {
    CheckpointOverBudget {
        checkpoint_bytes: usize,
        byte_limit: usize,
        compacted: CompactedHistory,
    },
    ContextOverflow {
        compacted: CompactedHistory,
    },
    NoProgress {
        compacted: CompactedHistory,
    },
}

impl ReplacementValidationError {
    fn compacted(&self) -> &CompactedHistory {
        match self {
            Self::CheckpointOverBudget { compacted, .. }
            | Self::ContextOverflow { compacted }
            | Self::NoProgress { compacted } => compacted,
        }
    }
}

fn validate_replacement(
    runtime: &WindowRolloverPolicyRuntime,
    checkpoint: &CanonicalCheckpoint,
    provider_messages: &[AgentModelMessage],
    source_tokens: u64,
    before_messages: usize,
    validation_mode: WindowRolloverValidationMode,
) -> Result<ValidatedCompaction, ReplacementValidationError> {
    let compacted = runtime.rebuild_with_compression_from_normalized(provider_messages, checkpoint);
    let checkpoint_bytes = checkpoint.as_str().len();
    if checkpoint_bytes > SESSION_NOTES_CHECKPOINT_MAX_BYTES {
        return Err(ReplacementValidationError::CheckpointOverBudget {
            checkpoint_bytes,
            byte_limit: SESSION_NOTES_CHECKPOINT_MAX_BYTES,
            compacted,
        });
    }
    if compacted.after_tokens >= runtime.policy.trigger_tokens {
        return Err(ReplacementValidationError::ContextOverflow { compacted });
    }
    if validation_mode.requires_progress()
        && compacted.after_tokens >= source_tokens
        && compacted.messages.len() >= before_messages
    {
        return Err(ReplacementValidationError::NoProgress { compacted });
    }
    Ok(ValidatedCompaction {
        compacted,
        before_messages,
        checkpoint_bytes,
    })
}

fn failure_metrics(
    source_tokens: u64,
    compacted: &CompactedHistory,
    before_messages: usize,
    validation_mode: WindowRolloverValidationMode,
) -> CompactionFailureMetrics {
    CompactionFailureMetrics {
        trigger: validation_mode.trigger_code(),
        before_tokens: source_tokens,
        after_tokens: compacted.after_tokens,
        before_messages,
        after_messages: compacted.messages.len(),
    }
}

fn commit_failed_event(
    turn_id: TurnId,
    duration_ms: u64,
    source_tokens: u64,
    after_tokens: u64,
    before_messages: usize,
    after_messages: usize,
    validation_mode: WindowRolloverValidationMode,
    error: &AxonError,
) -> AgentTurnEvent {
    compression_failed_event(
        turn_id,
        &format!("窗口归档提交失败，当前历史保持不变：{error:?}"),
        duration_ms,
        "commit_failed",
        CompactionFailureMetrics {
            trigger: validation_mode.trigger_code(),
            before_tokens: source_tokens,
            after_tokens,
            before_messages,
            after_messages,
        },
    )
}

fn compaction_completed_event(
    turn_id: TurnId,
    duration_ms: u64,
    metrics: CompactionCompletedMetrics,
) -> AgentTurnEvent {
    AgentTurnEvent::new(
        turn_id,
        AgentTurnEventKind::CompactionCompleted,
        format!(
            "记忆窗口切换完成 · 耗时 {}：切换前 {} tokens，切换后估算 {} tokens",
            format_duration_millis_compact(duration_ms),
            format_decimal_count(metrics.before_tokens),
            format_decimal_count(metrics.after_tokens)
        ),
    )
    .with_phase("history")
    .with_status("completed")
    .with_duration_ms(duration_ms)
    .with_metadata(serde_json::json!({
        "trigger": metrics.trigger,
        "beforeTokens": metrics.before_tokens,
        "afterTokens": metrics.after_tokens,
        "beforeMessages": metrics.before_messages,
        "afterMessages": metrics.after_messages,
        "checkpointBytes": metrics.checkpoint_bytes,
    }))
}

pub(super) fn compression_failed_event(
    turn_id: TurnId,
    summary: &str,
    duration_ms: u64,
    reason: &str,
    metrics: CompactionFailureMetrics,
) -> AgentTurnEvent {
    AgentTurnEvent::new(turn_id, AgentTurnEventKind::CompactionCompleted, summary)
        .with_phase("history")
        .with_status("failed")
        .with_duration_ms(duration_ms)
        .with_metadata(serde_json::json!({
            "reason": reason,
            "failure": reason,
            "trigger": metrics.trigger,
            "beforeTokens": metrics.before_tokens,
            "afterTokens": metrics.after_tokens,
            "beforeMessages": metrics.before_messages,
            "afterMessages": metrics.after_messages,
        }))
}

fn validation_failed_event(
    runtime: &WindowRolloverPolicyRuntime,
    source_tokens: u64,
    before_messages: usize,
    turn_id: TurnId,
    duration_ms: u64,
    validation_mode: WindowRolloverValidationMode,
    error: ReplacementValidationError,
) -> AgentTurnEvent {
    let compacted = error.compacted();
    let metrics = failure_metrics(source_tokens, compacted, before_messages, validation_mode);
    let (summary, reason) =
        validation_failure_details(runtime, compacted, source_tokens, before_messages, &error);
    compression_failed_event(turn_id, &summary, duration_ms, reason, metrics)
}

fn validation_failure_details(
    runtime: &WindowRolloverPolicyRuntime,
    compacted: &CompactedHistory,
    source_tokens: u64,
    before_messages: usize,
    error: &ReplacementValidationError,
) -> (String, &'static str) {
    match error {
        ReplacementValidationError::CheckpointOverBudget {
            checkpoint_bytes,
            byte_limit,
            ..
        } => (
            format!(
                "窗口交接事实超过大小上限：{} bytes > {} bytes",
                format_decimal_count(u64::try_from(*checkpoint_bytes).unwrap_or(u64::MAX)),
                format_decimal_count(u64::try_from(*byte_limit).unwrap_or(u64::MAX))
            ),
            "checkpoint_over_budget",
        ),
        ReplacementValidationError::ContextOverflow { .. } => (
            format!(
                "新窗口仍超过安全换窗线：估算 {} tokens ≥ 换窗线 {}",
                format_decimal_count(compacted.after_tokens),
                format_decimal_count(runtime.policy.trigger_tokens)
            ),
            ROLLOVER_FAILURE_CONTEXT_OVERFLOW,
        ),
        ReplacementValidationError::NoProgress { .. } => (
            format!(
                "自动换窗没有取得收敛：估算 {} -> {} tokens，消息数 {} -> {}",
                format_decimal_count(source_tokens),
                format_decimal_count(compacted.after_tokens),
                before_messages,
                compacted.messages.len()
            ),
            ROLLOVER_FAILURE_NO_PROGRESS,
        ),
    }
}
