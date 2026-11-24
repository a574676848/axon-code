use super::*;
use crate::session_notes::{
    SESSION_NOTES_CHECKPOINT_MAX_BYTES, SESSION_NOTES_CHECKPOINT_MAX_TOKENS,
};
use crate::token_estimator::estimated_text_tokens;

pub(super) struct CompactionCompletionContext<'a> {
    pub(super) runtime: &'a WindowRolloverPolicyRuntime,
    pub(super) checkpoint: &'a CanonicalCheckpoint,
    pub(super) turn_id: TurnId,
    pub(super) duration_ms: u64,
    pub(super) provider_messages: &'a [AgentModelMessage],
    pub(super) validation_mode: WindowRolloverValidationMode,
}

pub(super) struct CompactionFailureMetrics {
    pub(super) trigger: &'static str,
    pub(super) before_messages: usize,
    pub(super) after_messages: usize,
}

pub(super) struct CompactionCompletedMetrics {
    pub(super) trigger: &'static str,
    pub(super) before_messages: usize,
    pub(super) after_messages: usize,
    pub(super) checkpoint_bytes: usize,
    pub(super) checkpoint_tokens: u64,
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
        validate_replacement(
            self,
            checkpoint,
            provider_messages,
            provider_messages.len(),
            validation_mode,
        )
        .map(|validated| validated.compacted)
        .map_err(|error| {
            let (summary, _) =
                validation_failure_details(error.compacted(), provider_messages.len(), &error);
            AxonError::runtime_failure(summary)
        })
    }
}

pub(super) fn complete_compaction(
    context: CompactionCompletionContext<'_>,
    messages: &mut AgentModelHistorySnapshot,
    turn_events: &mut Vec<AgentTurnEvent>,
    transaction: &mut dyn WindowRolloverTransaction,
) -> CompressionOutcome {
    let validated = match validate_replacement(
        context.runtime,
        context.checkpoint,
        context.provider_messages,
        context.provider_messages.len(),
        context.validation_mode,
    ) {
        Ok(validated) => validated,
        Err(error) => {
            // 先把可读原因记到事务上，再 abort：abort 的失败投影只读这一份原因，
            // 否则事件流里只剩默认的「未通过门禁」，校验失败的具体事实被盖掉。
            let (summary, _) = validation_failure_details(
                error.compacted(),
                context.provider_messages.len(),
                &error,
            );
            transaction.note_failure_reason(&summary);
            transaction.abort();
            turn_events.push(validation_failed_event(
                context.provider_messages.len(),
                context.turn_id,
                context.duration_ms,
                context.validation_mode,
                error,
            ));
            return CompressionOutcome::Prompted;
        }
    };
    if let Err(error) = transaction.commit(
        context.provider_messages,
        context.checkpoint,
        &validated.compacted,
    ) {
        // 提交失败同样要先记原因：归档、Thread 投影、便签来源落库三步都可能单独失败，
        // 它们回滚后只留下一句默认文案就无法区分是哪一个。
        transaction.note_failure_reason(&format!("窗口归档提交失败，当前历史保持不变：{error:?}"));
        transaction.abort();
        turn_events.push(commit_failed_event(
            context.turn_id,
            context.duration_ms,
            validated.before_messages,
            validated.compacted.messages.len(),
            context.validation_mode,
            &error,
        ));
        // durable 投影失败时不能只改内存历史：窗口身份与归档都还停在旧窗口上，
        // 界面读数却会跟着新窗口一起变小，下一轮又要把整段旧历史重新装回来。
        // 保留原历史，让读数和持久事实指向同一个窗口；便签与换窗提示仍会下发，
        // 由模型在同一个窗口内重试（归档问题修好后即可一次成功）。
        return CompressionOutcome::Prompted;
    }

    let after_messages = validated.compacted.messages.len();
    *messages = validated.compacted.messages.into();
    turn_events.push(compaction_completed_event(
        context.turn_id,
        context.duration_ms,
        CompactionCompletedMetrics {
            trigger: context.validation_mode.trigger_code(),
            before_messages: validated.before_messages,
            after_messages,
            checkpoint_bytes: validated.checkpoint_bytes,
            checkpoint_tokens: estimated_text_tokens(context.checkpoint.as_str()),
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
        checkpoint_tokens: u64,
        token_limit: u64,
        compacted: CompactedHistory,
    },
    NoProgress {
        compacted: CompactedHistory,
    },
}

impl ReplacementValidationError {
    fn compacted(&self) -> &CompactedHistory {
        match self {
            Self::CheckpointOverBudget { compacted, .. } | Self::NoProgress { compacted } => {
                compacted
            }
        }
    }
}

fn validate_replacement(
    runtime: &WindowRolloverPolicyRuntime,
    checkpoint: &CanonicalCheckpoint,
    provider_messages: &[AgentModelMessage],
    before_messages: usize,
    validation_mode: WindowRolloverValidationMode,
) -> Result<ValidatedCompaction, ReplacementValidationError> {
    let compacted = runtime.rebuild_with_compression_from_normalized(provider_messages, checkpoint);
    let checkpoint_bytes = checkpoint.as_str().len();
    let checkpoint_tokens = estimated_text_tokens(checkpoint.as_str());
    if checkpoint_tokens > SESSION_NOTES_CHECKPOINT_MAX_TOKENS
        || checkpoint_bytes > SESSION_NOTES_CHECKPOINT_MAX_BYTES
    {
        return Err(ReplacementValidationError::CheckpointOverBudget {
            checkpoint_bytes,
            byte_limit: SESSION_NOTES_CHECKPOINT_MAX_BYTES,
            checkpoint_tokens,
            token_limit: SESSION_NOTES_CHECKPOINT_MAX_TOKENS,
            compacted,
        });
    }
    /*
     * 换窗后不再用本地估算复查「新窗口是否仍超线」：窗口体量只认 Provider 实测，
     * 没有实测值就不该用一个估算数把换窗判死。
     *
     * 自动路径仍要求取得收敛：消息数必须减少；只有消息数没减少且本地体量估算也没下降时
     * 才判 NoProgress。这里的估算只在判定内部使用，不进事件、不进界面。
     */
    if validation_mode.requires_progress()
        && compacted.messages.len() >= before_messages
        && estimated_history_tokens(compacted.messages.as_slice())
            >= estimated_history_tokens(provider_messages)
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
    compacted: &CompactedHistory,
    before_messages: usize,
    validation_mode: WindowRolloverValidationMode,
) -> CompactionFailureMetrics {
    CompactionFailureMetrics {
        trigger: validation_mode.trigger_code(),
        before_messages,
        after_messages: compacted.messages.len(),
    }
}

fn commit_failed_event(
    turn_id: TurnId,
    duration_ms: u64,
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
            "记忆窗口切换完成 · 耗时 {}",
            format_duration_millis_compact(duration_ms)
        ),
    )
    .with_phase("history")
    .with_status("completed")
    .with_duration_ms(duration_ms)
    .with_metadata(serde_json::json!({
        "trigger": metrics.trigger,
        "beforeMessages": metrics.before_messages,
        "afterMessages": metrics.after_messages,
        "checkpointBytes": metrics.checkpoint_bytes,
        "checkpointTokens": metrics.checkpoint_tokens,
        "checkpointTokenLimit": SESSION_NOTES_CHECKPOINT_MAX_TOKENS,
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
            "beforeMessages": metrics.before_messages,
            "afterMessages": metrics.after_messages,
        }))
}

fn validation_failed_event(
    before_messages: usize,
    turn_id: TurnId,
    duration_ms: u64,
    validation_mode: WindowRolloverValidationMode,
    error: ReplacementValidationError,
) -> AgentTurnEvent {
    let compacted = error.compacted();
    let metrics = failure_metrics(compacted, before_messages, validation_mode);
    let (summary, reason) = validation_failure_details(compacted, before_messages, &error);
    compression_failed_event(turn_id, &summary, duration_ms, reason, metrics)
}

fn validation_failure_details(
    compacted: &CompactedHistory,
    before_messages: usize,
    error: &ReplacementValidationError,
) -> (String, &'static str) {
    match error {
        ReplacementValidationError::CheckpointOverBudget {
            checkpoint_bytes,
            byte_limit,
            checkpoint_tokens,
            token_limit,
            ..
        } => (
            format!(
                "窗口交接事实超过预算：{} tokens > {} tokens，或 {} bytes > {} bytes；请压缩便签后重写交接摘要",
                format_decimal_count(*checkpoint_tokens),
                format_decimal_count(*token_limit),
                format_decimal_count(u64::try_from(*checkpoint_bytes).unwrap_or(u64::MAX)),
                format_decimal_count(u64::try_from(*byte_limit).unwrap_or(u64::MAX))
            ),
            "checkpoint_over_budget",
        ),
        ReplacementValidationError::NoProgress { .. } => (
            format!(
                "自动换窗没有取得收敛：消息数 {} -> {}",
                before_messages,
                compacted.messages.len()
            ),
            ROLLOVER_FAILURE_NO_PROGRESS,
        ),
    }
}
