use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, OnceLock},
    thread,
    time::Duration,
};

use axon_agent_runtime::CompressionPolicy;
use axon_config::AxonConfig;

use super::{
    super::run_compaction_notice::{
        CompactionNotice, CompactionNoticeStatus, NOTICE_LOG_PREFIX, session_data_dir,
        write_compaction_notice,
    },
    super::run_compaction_projection::{
        CompactionLifecycleView, compaction_part_id, completed_summary, emit_compaction_lifecycle,
        failed_summary, planned_summary,
    },
    super::run_execution::now_ms,
    super::run_progress::RunProgressSink,
    super::run_session_store::resolved_axon_root,
    super::run_thread_store::{load_thread_detail, thread_model_history_json},
    CompactionDecision, CompactionInstallReport, CompactionTrigger, RunCancelToken,
};

const IDLE_COMPACTION_TELEMETRY_EVENT: &str = "compaction";
const IDLE_COMPACTION_TELEMETRY_TRIGGER: &str = "idle";
const IDLE_COMPACTION_TELEMETRY_REASON: &str = "idle_soft_target";
const IDLE_COMPACTION_TELEMETRY_IMPLEMENTATION: &str = "local";
const IDLE_COMPACTION_TELEMETRY_PHASE: &str = "history";
const IDLE_COMPACTION_TELEMETRY_STRATEGY: &str = "window_rollover";
const IDLE_COMPACTION_STATUS_COMPLETED: &str = "completed";
const IDLE_COMPACTION_STATUS_SKIPPED: &str = "skipped";
const IDLE_COMPACTION_STATUS_INTERRUPTED: &str = "interrupted";
const IDLE_COMPACTION_STATUS_FAILED: &str = "failed";
const IDLE_COMPACTION_SKIP_POLICY_UNAVAILABLE: &str = "policy_unavailable";
const IDLE_COMPACTION_SKIP_HISTORY_CHANGED: &str = "history_changed";
const IDLE_COMPACTION_SKIP_INELIGIBLE: &str = "ineligible";
const IDLE_COMPACTION_INTERRUPTED_CANCELLED: &str = "cancelled";
const IDLE_COMPACTION_LOG_PREFIX: &str = "compaction_telemetry";

#[derive(Debug, Clone, Copy)]
pub(super) enum IdleCompactionTelemetryOutcome<'a> {
    Completed(&'a CompactionInstallReport),
    Skipped(&'a str),
    Interrupted(&'a str),
    Failed(&'a str),
}

pub(super) fn idle_compaction_telemetry(
    session_id: &str,
    started_at_ms: u64,
    completed_at_ms: u64,
    outcome: IdleCompactionTelemetryOutcome<'_>,
) -> serde_json::Value {
    let (status, skip_reason, error, report) = match outcome {
        IdleCompactionTelemetryOutcome::Completed(report) => {
            (IDLE_COMPACTION_STATUS_COMPLETED, None, None, Some(report))
        }
        IdleCompactionTelemetryOutcome::Skipped(reason) => {
            (IDLE_COMPACTION_STATUS_SKIPPED, Some(reason), None, None)
        }
        IdleCompactionTelemetryOutcome::Interrupted(reason) => {
            (IDLE_COMPACTION_STATUS_INTERRUPTED, Some(reason), None, None)
        }
        IdleCompactionTelemetryOutcome::Failed(error) => {
            (IDLE_COMPACTION_STATUS_FAILED, None, Some(error), None)
        }
    };
    let duration_ms = completed_at_ms.saturating_sub(started_at_ms);
    serde_json::json!({
        "event": IDLE_COMPACTION_TELEMETRY_EVENT,
        "session_id": session_id,
        "trigger": IDLE_COMPACTION_TELEMETRY_TRIGGER,
        "reason": IDLE_COMPACTION_TELEMETRY_REASON,
        "implementation": IDLE_COMPACTION_TELEMETRY_IMPLEMENTATION,
        "phase": IDLE_COMPACTION_TELEMETRY_PHASE,
        "strategy": IDLE_COMPACTION_TELEMETRY_STRATEGY,
        "status": status,
        "skip_reason": skip_reason,
        "error": error,
        "before_tokens": report.map(|value| value.before_tokens),
        "after_tokens": report.map(|value| value.after_tokens),
        "before_messages": report.map(|value| value.before_messages),
        "after_messages": report.map(|value| value.after_messages),
        "removed_items": report.map(|value| value.removed_items),
        "started_at_ms": started_at_ms,
        "completed_at_ms": completed_at_ms,
        "duration_ms": duration_ms,
    })
}

fn record_idle_compaction_telemetry(
    session_id: &str,
    started_at_ms: u64,
    outcome: IdleCompactionTelemetryOutcome<'_>,
) {
    let message = format!(
        "{IDLE_COMPACTION_LOG_PREFIX}={}",
        idle_compaction_telemetry(session_id, started_at_ms, now_ms(), outcome)
    );
    if matches!(outcome, IdleCompactionTelemetryOutcome::Failed(_)) {
        axon_logger::warning(&message);
    } else {
        axon_logger::info(&message);
    }
}

const IDLE_COMPACTION_MAX_TASKS: usize = 32;
const MIN_IDLE_COMPACTION_HISTORY_ITEMS: usize = 2;

pub(crate) struct IdleCompactionTask {
    pub(crate) token: RunCancelToken,
    pub(crate) last_used: u64,
}

pub(crate) struct IdleCompactionRegistry {
    pub(crate) tasks: HashMap<String, IdleCompactionTask>,
    pub(crate) clock: u64,
}

impl IdleCompactionRegistry {
    pub(crate) fn next_access(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    pub(crate) fn touch(&mut self, session_id: &str) -> bool {
        let access = self.next_access();
        let Some(task) = self.tasks.get_mut(session_id) else {
            return false;
        };
        task.last_used = access;
        true
    }
}

fn idle_compaction_tasks() -> &'static Mutex<IdleCompactionRegistry> {
    static TASKS: OnceLock<Mutex<IdleCompactionRegistry>> = OnceLock::new();
    TASKS.get_or_init(|| {
        Mutex::new(IdleCompactionRegistry {
            tasks: HashMap::new(),
            clock: 0,
        })
    })
}

pub(crate) fn cancel_idle_compaction(session_id: &str) {
    if let Ok(mut registry) = idle_compaction_tasks().lock() {
        registry.touch(session_id);
        if let Some(task) = registry.tasks.remove(session_id) {
            task.token.cancel();
        }
    }
}

pub(crate) fn schedule_idle_compaction(
    config_path: &Path,
    config: &AxonConfig,
    session_id: &str,
    progress: Option<&RunProgressSink<'_>>,
) {
    let root = match resolved_axon_root(config_path) {
        Ok(root) => root,
        Err(_) => return,
    };
    let detail = match load_thread_detail(&root, session_id) {
        Ok(detail) => detail,
        Err(_) => return,
    };
    let Some(selected) = idle_compaction_selection(config, &detail) else {
        return;
    };
    let Some(policy) = idle_compaction_policy_for_selection(config, &selected) else {
        return;
    };
    let Ok(current_context_tokens) =
        super::current_context_tokens_for_selection(config_path, &detail, &selected)
    else {
        return;
    };
    if !idle_compaction_eligible(&detail, policy, current_context_tokens) {
        return;
    }

    // 排期事实必须在本 turn 的投影通道内发出：真正执行发生在 debounce 之后，
    // 那时该 turn 的通道已经关闭，只能由结果通知账本在下一次 turn 补投。
    emit_compaction_lifecycle(
        progress,
        &CompactionLifecycleView::planned(
            compaction_part_id(CompactionTrigger::Idle, session_id),
            planned_summary(
                current_context_tokens,
                policy.trigger_tokens,
                config.agent.idle_compaction_debounce_ms,
            ),
        ),
    );

    let expected_turn_sequence = detail.turns.last().map(|turn| turn.sequence).unwrap_or(0);
    let token = RunCancelToken::default();
    let child = token.child_token();
    {
        let Ok(mut registry) = idle_compaction_tasks().lock() else {
            return;
        };
        if registry.tasks.contains_key(session_id) {
            registry.touch(session_id);
        }
        let last_used = registry.next_access();
        if let Some(previous) = registry.tasks.insert(
            session_id.to_owned(),
            IdleCompactionTask {
                token: token.clone(),
                last_used,
            },
        ) {
            previous.token.cancel();
        }
        if registry.tasks.len() > IDLE_COMPACTION_MAX_TASKS {
            let lru_session = registry
                .tasks
                .iter()
                .filter(|(id, _)| id.as_str() != session_id)
                .min_by_key(|(_, task)| task.last_used)
                .map(|(id, _)| id.clone());
            if let Some(lru_session) = lru_session
                && let Some(evicted) = registry.tasks.remove(&lru_session)
            {
                evicted.token.cancel();
            }
        }
    }

    let config_path = config_path.to_path_buf();
    let config = config.clone();
    let session_id = session_id.to_owned();
    let session_dir = session_data_dir(&root, &session_id);
    let task_token = token.clone();
    let debounce_ms = config.agent.idle_compaction_debounce_ms;
    thread::spawn(move || {
        let debounce_started_at_ms = now_ms();
        if child.wait_timeout(Duration::from_millis(debounce_ms)) {
            record_idle_compaction_telemetry(
                &session_id,
                debounce_started_at_ms,
                IdleCompactionTelemetryOutcome::Interrupted(IDLE_COMPACTION_INTERRUPTED_CANCELLED),
            );
        } else {
            let attempt_started_at_ms = now_ms();
            match compact_session_with_expected(
                &config_path,
                &config,
                &session_id,
                expected_turn_sequence,
                &child,
                selected,
            ) {
                Ok(CompactionDecision::Installed(report)) => {
                    record_idle_compaction_telemetry(
                        &session_id,
                        attempt_started_at_ms,
                        IdleCompactionTelemetryOutcome::Completed(&report),
                    );
                    write_completed_notice(
                        &session_dir,
                        &session_id,
                        &report,
                        completed_summary(
                            CompactionTrigger::Idle,
                            report.before_tokens,
                            report.after_tokens,
                            report.before_messages,
                            report.after_messages,
                            report.duration_ms,
                        ),
                        now_ms(),
                    );
                }
                Ok(CompactionDecision::Skipped(reason)) => {
                    record_idle_compaction_telemetry(
                        &session_id,
                        attempt_started_at_ms,
                        IdleCompactionTelemetryOutcome::Skipped(reason),
                    );
                }
                Err(error) if error == super::COMPACTION_CANCELLED_ERROR => {
                    record_idle_compaction_telemetry(
                        &session_id,
                        attempt_started_at_ms,
                        IdleCompactionTelemetryOutcome::Interrupted(
                            IDLE_COMPACTION_INTERRUPTED_CANCELLED,
                        ),
                    );
                }
                Err(error) => {
                    record_idle_compaction_telemetry(
                        &session_id,
                        attempt_started_at_ms,
                        IdleCompactionTelemetryOutcome::Failed(&error),
                    );
                    write_failed_notice(
                        &session_dir,
                        &session_id,
                        failed_summary(CompactionTrigger::Idle, &error),
                        now_ms(),
                    );
                }
            }
        }
        if let Ok(mut registry) = idle_compaction_tasks().lock() {
            let is_current = registry
                .tasks
                .get(&session_id)
                .is_some_and(|current| current.token.same_execution(&task_token));
            if is_current {
                registry.tasks.remove(&session_id);
            }
        }
    });
}

/// 记录成功结果；通知写失败只降级为告警，压缩本身已经提交，不允许回滚已完成的轮转。
fn write_completed_notice(
    session_dir: &Path,
    session_id: &str,
    report: &CompactionInstallReport,
    summary: String,
    recorded_at_ms: u64,
) {
    let rollover = report.context_rollover();
    write_notice_best_effort(
        session_dir,
        &CompactionNotice {
            session_id: session_id.to_owned(),
            status: CompactionNoticeStatus::Completed,
            summary,
            window_number: Some(rollover.window_number),
            window_id: Some(rollover.window_id),
            previous_window_id: rollover.previous_window_id,
            archived_messages_count: rollover.archived_messages_count,
            notes_injected: rollover.notes_injected,
            duration_ms: report.duration_ms,
            recorded_at_ms,
        },
    );
}

/// 记录失败结果；失败不替换历史，但必须让用户在下一次 run 看到原因。
fn write_failed_notice(session_dir: &Path, session_id: &str, summary: String, recorded_at_ms: u64) {
    write_notice_best_effort(
        session_dir,
        &CompactionNotice {
            session_id: session_id.to_owned(),
            status: CompactionNoticeStatus::Failed,
            summary,
            window_number: None,
            window_id: None,
            previous_window_id: None,
            archived_messages_count: 0,
            notes_injected: false,
            duration_ms: 0,
            recorded_at_ms,
        },
    );
}

fn write_notice_best_effort(session_dir: &Path, notice: &CompactionNotice) {
    if let Err(error) = write_compaction_notice(session_dir, notice) {
        axon_logger::warning(&format!("{NOTICE_LOG_PREFIX} {error}"));
    }
}

fn idle_compaction_selection(
    config: &AxonConfig,
    detail: &axon_session::ThreadDetail,
) -> Option<super::super::run_model_routing::ChannelBalancedModel> {
    super::super::run_request::resolved_request_channel(
        config,
        &detail.thread.settings.provider,
        &detail.thread.settings.model,
    )
    .ok()
}

fn idle_compaction_policy_for_selection(
    config: &AxonConfig,
    selected: &super::super::run_model_routing::ChannelBalancedModel,
) -> Option<CompressionPolicy> {
    CompressionPolicy::for_context_window(
        selected.model.context_window,
        config.agent.auto_compaction_threshold,
    )
    .ok()
    .flatten()
}

pub(crate) fn idle_compaction_eligible(
    detail: &axon_session::ThreadDetail,
    policy: CompressionPolicy,
    current_context_tokens: u64,
) -> bool {
    if detail.thread.status != axon_session::ThreadStatus::Idle
        || detail
            .turns
            .last()
            .is_some_and(|turn| turn.status == axon_session::TurnStatus::Active)
        || super::compacted_after_latest_turn(detail)
    {
        return false;
    }
    if current_context_tokens < policy.trigger_tokens {
        return false;
    }
    let Ok(history_json) = thread_model_history_json(detail) else {
        return false;
    };
    let Ok(history) =
        serde_json::from_str::<Vec<axon_agent_runtime::AgentModelMessage>>(&history_json)
    else {
        return false;
    };
    history.len() >= MIN_IDLE_COMPACTION_HISTORY_ITEMS
}

fn compact_session_with_expected(
    config_path: &Path,
    config: &AxonConfig,
    session_id: &str,
    expected_turn_sequence: u64,
    cancel_token: &RunCancelToken,
    selected: super::super::run_model_routing::ChannelBalancedModel,
) -> Result<CompactionDecision, String> {
    let root = resolved_axon_root(config_path)?;
    let detail = load_thread_detail(&root, session_id)?;
    let Some(policy) = idle_compaction_policy_for_selection(config, &selected) else {
        return Ok(CompactionDecision::Skipped(
            IDLE_COMPACTION_SKIP_POLICY_UNAVAILABLE,
        ));
    };
    if detail.turns.last().map(|turn| turn.sequence) != Some(expected_turn_sequence) {
        return Ok(CompactionDecision::Skipped(
            IDLE_COMPACTION_SKIP_HISTORY_CHANGED,
        ));
    }
    let current_context_tokens =
        super::current_context_tokens_for_selection(config_path, &detail, &selected)?;
    if !idle_compaction_eligible(&detail, policy, current_context_tokens) {
        return Ok(CompactionDecision::Skipped(IDLE_COMPACTION_SKIP_INELIGIBLE));
    }
    super::compact_loaded_thread(
        config_path,
        config,
        &detail,
        CompactionTrigger::Idle,
        Some(cancel_token),
        selected,
    )
}
