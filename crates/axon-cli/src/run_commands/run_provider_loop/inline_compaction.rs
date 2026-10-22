//! 循环内代际轮转与战术笔记状态提取（CLI run adapter）。
//!
//! 通过本地 SessionNotes 完成窗口轮转事实交接：
//! 1. 不发起远程模型请求，不产生额外模型 Token；
//! 2. prepare 阶段只生成结构化 checkpoint；
//! 3. replacement history 校验通过后，commit 阶段才归档并推进窗口。

use std::path::PathBuf;

use axon_agent_runtime::{
    AgentModelMessage, CanonicalCheckpoint, CompactedHistory, DEFAULT_WINDOW_ID, SessionNotes,
    SessionNotesRuntime, WindowArchiveResult, WindowRolloverEngine, WindowRolloverTransaction,
    WindowRolloverValidationMode,
};
use axon_domain::{AxonError, AxonResult};

use super::super::RunRequest;
use super::super::run_compaction_projection::{
    CompactionLifecycleView, CompactionTrigger, compaction_part_id, completed_summary,
    context_rollover_fact, emit_compaction_lifecycle, emit_context_rollover, failed_summary,
    started_summary,
};
use super::super::run_execution::now_ms;
use super::super::run_progress::RunProgressSink;
use super::super::run_session_store::resolved_axon_root;

/// 门禁未通过且没有更具体原因时的失败说明。
const INLINE_ROLLOVER_ABORTED_REASON: &str = "窗口轮转未通过门禁";
/// 内联轮转的 replacement history 必然携带 checkpoint 交接投影，否则 publish 会被拒绝。
const CHECKPOINT_INJECTED_INTO_NEW_WINDOW: bool = true;

struct PreparedInlineWindowRollover {
    session_dir: PathBuf,
    current_window_number: u64,
    previous_window_id: Option<String>,
    expected_revision: u64,
    lifecycle_id: String,
    checkpoint: CanonicalCheckpoint,
    started_at_ms: u64,
    persisted_at_ms: i64,
}

struct CommittedInlineWindowRollover {
    started_at_ms: u64,
    rollover: WindowArchiveResult,
    before_tokens: u64,
    after_tokens: u64,
    before_messages: usize,
    after_messages: usize,
}

/// Provider loop 的两阶段窗口轮转事务。
pub(super) struct InlineWindowRollover<'request, 'sink> {
    request: &'request RunRequest,
    progress: Option<&'request RunProgressSink<'sink>>,
    prepared: Option<PreparedInlineWindowRollover>,
    committed: Option<CommittedInlineWindowRollover>,
    active_started_at_ms: Option<u64>,
    active_failure_reason: Option<String>,
    active_through_sequence: Option<u64>,
    active_before_tokens: u64,
    active_checkpoint_text: String,
}

impl<'request, 'sink> InlineWindowRollover<'request, 'sink> {
    pub(super) fn new(
        request: &'request RunRequest,
        progress: Option<&'request RunProgressSink<'sink>>,
    ) -> Self {
        Self {
            request,
            progress,
            prepared: None,
            committed: None,
            active_started_at_ms: None,
            active_failure_reason: None,
            active_through_sequence: None,
            active_before_tokens: 0,
            active_checkpoint_text: String::new(),
        }
    }

    fn build_prepared(
        &self,
        messages: &[AgentModelMessage],
        started_at_ms: u64,
    ) -> AxonResult<(CanonicalCheckpoint, PreparedInlineWindowRollover)> {
        let root = resolved_axon_root(&self.request.config_path).map_err(|error| {
            AxonError::runtime_failure(format!("解析 Axon 根目录失败: {error}"))
        })?;
        let session_dir = root.join("sessions").join(&self.request.session_id);
        let persisted_at_ms = i64::try_from(started_at_ms).unwrap_or(i64::MAX);
        let user_objective = messages
            .iter()
            .find(|message| message.is_real_user_turn())
            .map_or(self.request.prompt.as_str(), |message| {
                message.content.as_str()
            });
        SessionNotesRuntime::initialize_goal_anchor(&session_dir, user_objective, persisted_at_ms)?;
        let notes = SessionNotes::load_from_session(&session_dir)?
            .unwrap_or_else(|| SessionNotes::new(Default::default(), persisted_at_ms));
        let summary = notes.try_render_canonical_summary(&session_dir, messages)?;
        let checkpoint = CanonicalCheckpoint::parse(&summary, messages).map_err(|error| {
            AxonError::runtime_failure(format!("窗口轮转 checkpoint 无效：{error}"))
        })?;
        let plan = PreparedInlineWindowRollover {
            session_dir,
            current_window_number: notes.window_number,
            previous_window_id: notes.current_window_id,
            expected_revision: notes.revision,
            lifecycle_id: format!(
                "inline-rollover:{}:{}",
                self.request.session_id, notes.window_number
            ),
            checkpoint: checkpoint.clone(),
            started_at_ms,
            persisted_at_ms,
        };
        Ok((checkpoint, plan))
    }
}

impl WindowRolloverTransaction for InlineWindowRollover<'_, '_> {
    fn prepare(&mut self, messages: &[AgentModelMessage]) -> AxonResult<CanonicalCheckpoint> {
        if super::run_cancelled(self.request) {
            return Err(AxonError::runtime_failure("agent_run_interrupted"));
        }
        if self.prepared.is_some() {
            return Err(AxonError::runtime_failure(
                "窗口轮转事务已有待提交 checkpoint",
            ));
        }

        let started_at_ms = now_ms();
        self.active_failure_reason = None;
        emit_compaction_lifecycle(
            self.progress,
            &CompactionLifecycleView::started(
                compaction_part_id(CompactionTrigger::Inline, &self.request.session_id),
                started_summary(CompactionTrigger::Inline),
                started_at_ms,
            ),
        );
        self.active_started_at_ms = Some(started_at_ms);
        let (checkpoint, prepared) = self.build_prepared(messages, started_at_ms)?;
        self.prepared = Some(prepared);
        Ok(checkpoint)
    }

    fn validate_memory_handoff(
        &mut self,
        _validation_mode: WindowRolloverValidationMode,
    ) -> AxonResult<()> {
        let result = (|| {
            let root = resolved_axon_root(&self.request.config_path).map_err(|error| {
                AxonError::runtime_failure(format!("解析 Axon 根目录失败: {error}"))
            })?;
            let session_dir = root.join("sessions").join(&self.request.session_id);
            let notes = SessionNotes::load_from_session(&session_dir)?
                .ok_or_else(|| AxonError::runtime_failure("会话便签尚未初始化"))?;
            SessionNotesRuntime::ensure_state_saved_for_window(&session_dir, notes.window_number)
        })();
        if let Err(error) = &result {
            // 门禁发生在 prepare 之前，必须在这里补记开始时刻与失败原因，
            // 否则随后到来的 abort() 会因为「没有进行中的轮转」而不投影任何事实。
            self.active_started_at_ms.get_or_insert_with(now_ms);
            self.active_failure_reason = Some(format!("换窗前任务状态未完成保存：{error:?}"));
        }
        result
    }

    fn current_window_id(&mut self) -> Option<String> {
        let root = resolved_axon_root(&self.request.config_path).ok()?;
        let session_dir = root.join("sessions").join(&self.request.session_id);
        let notes = SessionNotes::load_from_session(&session_dir)
            .ok()
            .flatten()?;
        Some(
            notes
                .current_window_id
                .unwrap_or_else(|| DEFAULT_WINDOW_ID.to_owned()),
        )
    }

    fn commit(
        &mut self,
        messages: &[AgentModelMessage],
        checkpoint: &CanonicalCheckpoint,
        replacement: &CompactedHistory,
    ) -> AxonResult<()> {
        if super::run_cancelled(self.request) {
            return Err(AxonError::runtime_failure("agent_run_interrupted"));
        }
        let prepared = self
            .prepared
            .take()
            .ok_or_else(|| AxonError::runtime_failure("窗口轮转事务缺少待提交 checkpoint"))?;
        if prepared.checkpoint != *checkpoint {
            return Err(AxonError::runtime_failure(
                "窗口轮转 checkpoint 在提交前发生变化",
            ));
        }

        let compaction = axon_session::ThreadCompaction {
            history_json: serde_json::to_string(&replacement.messages).map_err(|error| {
                AxonError::runtime_failure(format!("活动窗口编码失败：{error}"))
            })?,
            checkpoint_text: checkpoint.as_str().to_owned(),
            through_sequence: 0,
            before_tokens: axon_agent_runtime::estimated_history_tokens(messages),
            after_tokens: replacement.after_tokens,
            recorded_at_ms: now_ms(),
        };
        let before_tokens = compaction.before_tokens;
        let (through_sequence, rollover) =
            self.commit_projection(&prepared, messages, compaction)?;
        self.active_through_sequence = Some(through_sequence);
        self.active_before_tokens = before_tokens;
        self.active_checkpoint_text = checkpoint.as_str().to_owned();
        self.committed = Some(CommittedInlineWindowRollover {
            started_at_ms: prepared.started_at_ms,
            rollover,
            before_tokens,
            after_tokens: replacement.after_tokens,
            before_messages: messages.len(),
            after_messages: replacement.messages.len(),
        });
        Ok(())
    }

    fn complete(&mut self) {
        let Some(committed) = self.committed.take() else {
            return;
        };
        let duration_ms = committed.rollover.duration_ms;
        emit_compaction_lifecycle(
            self.progress,
            &CompactionLifecycleView::completed(
                compaction_part_id(CompactionTrigger::Inline, &self.request.session_id),
                completed_summary(
                    CompactionTrigger::Inline,
                    committed.before_tokens,
                    committed.after_tokens,
                    committed.before_messages,
                    committed.after_messages,
                    duration_ms,
                ),
                Some(committed.started_at_ms),
                Some(duration_ms),
            ),
        );
        emit_context_rollover(
            self.progress,
            &context_rollover_fact(
                committed.rollover.window_number,
                committed.rollover.window_id.clone(),
                committed.rollover.previous_window_id.clone(),
                committed.rollover.archived_messages_count,
                CHECKPOINT_INJECTED_INTO_NEW_WINDOW,
            ),
        );
        self.active_started_at_ms = None;
    }

    fn publish_active_window(&mut self, messages: &[AgentModelMessage]) -> AxonResult<()> {
        let Some(through_sequence) = self.active_through_sequence else {
            return Ok(());
        };
        let history_json = serde_json::to_string(messages)
            .map_err(|error| AxonError::runtime_failure(format!("活动窗口序列化失败：{error}")))?;
        if !messages.iter().any(|message| {
            matches!(
                message.kind,
                axon_agent_runtime::AgentModelMessageKind::ConversationCheckpoint
            )
        }) {
            return Err(AxonError::runtime_failure("活动窗口缺少便签交接投影"));
        }
        let checkpoint = axon_session::ThreadCompaction {
            history_json,
            checkpoint_text: self.active_checkpoint_text.clone(),
            through_sequence,
            before_tokens: self.active_before_tokens,
            after_tokens: axon_agent_runtime::estimated_history_tokens(messages),
            recorded_at_ms: now_ms(),
        };
        super::super::run_rollout_store::install_rollout_compaction(
            &self.request.config_path,
            &self.request.session_id,
            &checkpoint,
        )
        .map_err(AxonError::runtime_failure)
    }

    fn abort(&mut self) {
        self.prepared = None;
        self.committed = None;
        let Some(started_at_ms) = self.active_started_at_ms.take() else {
            return;
        };
        let reason = self
            .active_failure_reason
            .take()
            .unwrap_or_else(|| INLINE_ROLLOVER_ABORTED_REASON.to_owned());
        emit_compaction_lifecycle(
            self.progress,
            &CompactionLifecycleView::failed(
                compaction_part_id(CompactionTrigger::Inline, &self.request.session_id),
                failed_summary(CompactionTrigger::Inline, &reason),
                Some(started_at_ms),
                Some(now_ms().saturating_sub(started_at_ms)),
            ),
        );
    }
}

impl InlineWindowRollover<'_, '_> {
    fn commit_projection(
        &self,
        prepared: &PreparedInlineWindowRollover,
        messages: &[AgentModelMessage],
        compaction: axon_session::ThreadCompaction,
    ) -> AxonResult<(u64, WindowArchiveResult)> {
        let root =
            resolved_axon_root(&self.request.config_path).map_err(AxonError::runtime_failure)?;
        let expected_sequence = super::super::run_thread_store::thread_event_sequence_snapshot(
            &root,
            &self.request.session_id,
        )
        .map_err(AxonError::runtime_failure)?;
        let previous_notes = SessionNotes::load_from_session(&prepared.session_dir)?;
        let previous_archive = read_archive_snapshot(prepared)?;
        let rollover = WindowRolloverEngine::archive_active_window(
            &prepared.session_dir,
            prepared.current_window_number,
            prepared.previous_window_id.clone(),
            prepared.expected_revision,
            &prepared.lifecycle_id,
            messages,
            prepared.persisted_at_ms,
        )?;
        let projection = super::super::run_thread_store::install_active_compaction_projection(
            &self.request.config_path,
            &root,
            &self.request.session_id,
            &super::super::run_session_store::run_persistence_assistant_part_id(self.request),
            expected_sequence,
            compaction,
        );
        match projection {
            Ok(through_sequence) => Ok((through_sequence, rollover)),
            Err(error) => {
                let rollback = WindowRolloverEngine::rollback_active_window(
                    &prepared.session_dir,
                    prepared.current_window_number,
                    &prepared.lifecycle_id,
                    rollover.notes_revision,
                    previous_notes.as_ref(),
                    previous_archive.as_deref(),
                );
                Err(AxonError::runtime_failure(match rollback {
                    Ok(()) => format!("活动窗口投影失败，归档和窗口身份已回滚：{error}"),
                    Err(rollback_error) => format!(
                        "活动窗口投影失败且窗口回滚失败：projection={error}，rollback={rollback_error:?}"
                    ),
                }))
            }
        }
    }
}

fn read_archive_snapshot(prepared: &PreparedInlineWindowRollover) -> AxonResult<Option<Vec<u8>>> {
    WindowRolloverEngine::read_archive_snapshot(
        &prepared.session_dir,
        prepared.current_window_number,
    )
}

#[cfg(test)]
#[path = "inline_compaction_tests.rs"]
mod tests;
