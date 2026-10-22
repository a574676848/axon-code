//! Thread 投影。

use std::collections::BTreeMap;

use crate::domain::{
    Thread, ThreadDetail, ThreadEvent, ThreadEventRecord, ThreadStatus, ThreadSummary,
};

/// 从事件流构建 thread 详情。
pub(crate) fn project_threads(records: &[ThreadEventRecord]) -> Vec<ThreadDetail> {
    let mut details = BTreeMap::<String, ThreadDetail>::new();
    for record in records {
        apply_record(&mut details, record);
    }
    details.into_values().collect()
}

/// 构建摘要。
pub(crate) fn to_summary(detail: &ThreadDetail) -> ThreadSummary {
    ThreadSummary {
        thread_id: detail.thread.thread_id.clone(),
        parent_thread_id: detail.thread.parent_thread_id.clone(),
        forked_from_thread_id: detail.thread.forked_from_thread_id.clone(),
        created_at_ms: detail.thread.created_at_ms,
        updated_at_ms: detail.thread.updated_at_ms,
        name: detail.thread.name.clone(),
        provider: detail.thread.settings.provider.clone(),
        model: detail.thread.settings.model.clone(),
        cwd: detail.thread.settings.cwd.clone(),
        status: detail.thread.status,
        turn_count: detail.turns.len(),
    }
}

fn apply_record(details: &mut BTreeMap<String, ThreadDetail>, record: &ThreadEventRecord) {
    match &record.event {
        ThreadEvent::ThreadStarted {
            session_root_id,
            parent_thread_id,
            forked_from_thread_id,
            source,
            ephemeral,
            settings,
        } => apply_thread_started(
            details,
            ThreadDetail {
                thread: Thread {
                    thread_id: record.thread_id.clone(),
                    session_root_id: session_root_id.clone(),
                    parent_thread_id: parent_thread_id.clone(),
                    forked_from_thread_id: forked_from_thread_id.clone(),
                    name: None,
                    status: ThreadStatus::Idle,
                    source: *source,
                    ephemeral: *ephemeral,
                    settings: settings.clone(),
                    created_at_ms: record.recorded_at_ms,
                    updated_at_ms: record.recorded_at_ms,
                    archived_at_ms: None,
                },
                turns: Vec::new(),
                compacted_history: None,
            },
        ),
        ThreadEvent::TurnStarted { turn } => apply_turn_started(details, record, turn.clone()),
        ThreadEvent::TurnItemsAppended { turn_id, items } => {
            apply_turn_items_appended(details, record, turn_id, items);
        }
        ThreadEvent::TurnCompleted { turn } => apply_turn_completed(details, record, turn.clone()),
        ThreadEvent::TurnInterrupted { turn_id } => {
            apply_turn_interrupted(details, record, turn_id);
        }
        ThreadEvent::TurnFailed { turn_id, failure } => {
            apply_turn_failed(details, record, turn_id, failure);
        }
        ThreadEvent::ThreadResumed | ThreadEvent::ThreadUnarchived => {
            update_status(details, record, ThreadStatus::Idle);
        }
        ThreadEvent::ThreadCompacted { compaction } => {
            apply_thread_compacted(details, record, compaction.clone());
        }
        ThreadEvent::ThreadCheckpoint { detail } => {
            details.insert(detail.thread.thread_id.clone(), detail.clone());
        }
        ThreadEvent::ThreadForked { .. } => update_timestamp(details, record),
        ThreadEvent::ThreadNamed { name } => apply_thread_named(details, record, name.as_ref()),
        ThreadEvent::ThreadArchived => apply_thread_archived(details, record),
        ThreadEvent::ThreadClosed => update_status(details, record, ThreadStatus::NotLoaded),
    }
}

fn apply_thread_started(details: &mut BTreeMap<String, ThreadDetail>, detail: ThreadDetail) {
    details.insert(detail.thread.thread_id.clone(), detail);
}

fn apply_turn_started(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    turn: crate::domain::Turn,
) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        detail.turns.push(turn);
        detail.thread.status = ThreadStatus::Active;
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn apply_turn_completed(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    turn: crate::domain::Turn,
) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        let active = turn.status == crate::domain::TurnStatus::Active;
        if let Some(existing) = detail
            .turns
            .iter_mut()
            .find(|existing| existing.turn_id == turn.turn_id)
        {
            *existing = turn;
        } else {
            detail.turns.push(turn);
        }
        detail.thread.status = if active {
            ThreadStatus::Active
        } else {
            ThreadStatus::Idle
        };
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn apply_turn_items_appended(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    turn_id: &str,
    items: &[crate::domain::ThreadHistoryItem],
) {
    if let Some(detail) = details.get_mut(&record.thread_id)
        && let Some(turn) = detail.turns.iter_mut().find(|turn| turn.turn_id == turn_id)
    {
        turn.history_items.extend(items.iter().cloned());
        detail.thread.status = ThreadStatus::Active;
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn apply_turn_interrupted(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    turn_id: &str,
) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        if let Some(turn) = detail.turns.iter_mut().find(|turn| turn.turn_id == turn_id) {
            turn.status = crate::domain::TurnStatus::Interrupted;
            turn.interrupted_at_ms = Some(record.recorded_at_ms);
        }
        detail.thread.status = ThreadStatus::Idle;
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn apply_turn_failed(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    turn_id: &str,
    failure: &axon_domain::RuntimeFailure,
) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        if let Some(turn) = detail.turns.iter_mut().find(|turn| turn.turn_id == turn_id) {
            turn.status = crate::domain::TurnStatus::Failed;
            turn.completed_at_ms = Some(record.recorded_at_ms);
            turn.failure = Some(failure.clone());
        }
        detail.thread.status = ThreadStatus::Idle;
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn apply_thread_compacted(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    compaction: crate::domain::ThreadCompaction,
) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        detail.compacted_history = Some(compaction);
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn update_status(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    status: ThreadStatus,
) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        detail.thread.status = status;
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn update_timestamp(details: &mut BTreeMap<String, ThreadDetail>, record: &ThreadEventRecord) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn apply_thread_named(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    name: Option<&String>,
) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        detail.thread.name = name.cloned();
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn apply_thread_archived(details: &mut BTreeMap<String, ThreadDetail>, record: &ThreadEventRecord) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        detail.thread.status = ThreadStatus::Archived;
        detail.thread.archived_at_ms = Some(record.recorded_at_ms);
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}
