//! Thread 投影。

use std::collections::BTreeMap;

use crate::domain::{
    Thread, ThreadDetail, ThreadEvent, ThreadEventRecord, ThreadStatus, ThreadSummary,
};

/// 从事件流构建 thread 详情。
pub fn project_threads(records: &[ThreadEventRecord]) -> Vec<ThreadDetail> {
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
        child_origin: detail.thread.child_origin.clone(),
        last_turn_status: detail.turns.last().map(|turn| turn.status),
        last_turn_at_ms: detail.turns.last().map(|turn| {
            turn.completed_at_ms
                .or(turn.interrupted_at_ms)
                .unwrap_or(turn.started_at_ms)
        }),
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
                    child_origin: None,
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
        ThreadEvent::TurnInterrupted {
            turn_id,
            appended_items,
        } => {
            apply_turn_interrupted(details, record, turn_id, appended_items);
        }
        ThreadEvent::TurnFailed { turn_id, failure } => {
            apply_turn_failed(details, record, turn_id, failure);
        }
        ThreadEvent::TurnMetadataPatched {
            assistant_part_id,
            notice,
            affect_sequence,
        } => apply_turn_metadata_patched(
            details,
            record,
            assistant_part_id,
            notice.as_deref(),
            affect_sequence,
        ),
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
        ThreadEvent::ThreadAnnotated { child_origin } => {
            apply_thread_annotated(details, record, child_origin.clone());
        }
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
        // 中途落盘按稳定身份增量合并：历史 item 的生成顺序不是「只追加尾部」
        // （工具回执插在调用之间、reasoning 排在最后），按位置切片会写坏历史。
        // 同一身份后到的 item 整体替换原位，新身份追加到末尾。
        for item in items {
            match turn
                .history_items
                .iter_mut()
                .find(|existing| existing.identity() == item.identity())
            {
                Some(existing) => *existing = item.clone(),
                None => turn.history_items.push(item.clone()),
            }
        }
        detail.thread.status = ThreadStatus::Active;
        detail.thread.updated_at_ms = record.recorded_at_ms;
    }
}

fn apply_turn_interrupted(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    turn_id: &str,
    appended_items: &[crate::ThreadHistoryItem],
) {
    if let Some(detail) = details.get_mut(&record.thread_id)
        && let Some(turn) = detail.turns.iter_mut().find(|turn| turn.turn_id == turn_id)
    {
        turn.status = crate::domain::TurnStatus::Interrupted;
        turn.interrupted_at_ms = Some(record.recorded_at_ms);
        // 合成结果与终态同批落库，投影侧必须一起收进本轮历史，否则 JSONL 与
        // ThreadDetail 会对「本轮有哪些工具结果」给出两个答案。
        //
        // 按 part_id 去重：同一轮被重复收口时，重复追加会让同一批工具结果出现两次。
        for item in appended_items {
            if let crate::ThreadHistoryItem::ToolResult { part_id, .. } = item
                && turn.history_items.iter().any(|existing| {
                    matches!(
                        existing,
                        crate::ThreadHistoryItem::ToolResult { part_id: known, .. }
                            if known == part_id
                    )
                })
            {
                continue;
            }
            turn.history_items.push(item.clone());
        }
        // 只有真的找到该轮次，才说明这一线程的状态该回落到空闲；找不到时保持原状，
        // 避免把另一条仍在推进的轮次误标成空闲。
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

fn apply_thread_annotated(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    child_origin: Option<String>,
) {
    if let Some(detail) = details.get_mut(&record.thread_id) {
        detail.thread.child_origin = child_origin;
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

/// 套用治理回写增量：只改该轮 metadata 的两个字段，轮次状态与其余字段一律不动。
///
/// 找轮次的判据是 metadata 里的 `assistantPartId`：它是轮次在 thread 里的稳定身份，
/// 与 `turn_id`（由 part 派生）互为冗余，但 metadata 在读写两侧都存在。
fn apply_turn_metadata_patched(
    details: &mut BTreeMap<String, ThreadDetail>,
    record: &ThreadEventRecord,
    assistant_part_id: &str,
    notice: Option<&str>,
    affect_sequence: &[String],
) {
    let Some(detail) = details.get_mut(&record.thread_id) else {
        return;
    };
    let Some(turn) = detail
        .turns
        .iter_mut()
        .find(|turn| turn_metadata_has_part(turn.token_usage_json.as_deref(), assistant_part_id))
    else {
        return;
    };
    let Some(raw) = turn.token_usage_json.as_deref() else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(notice) = notice {
        object.insert(
            "reflectionNotice".to_owned(),
            serde_json::Value::String(notice.to_owned()),
        );
    }
    if !affect_sequence.is_empty()
        && let Ok(sequence) = serde_json::to_value(affect_sequence)
    {
        object.insert("affectSequence".to_owned(), sequence);
    }
    turn.token_usage_json = serde_json::to_string(&value).ok();
    detail.thread.updated_at_ms = record.recorded_at_ms;
}

/// 轮次 metadata 是否属于指定 assistant part。
fn turn_metadata_has_part(raw: Option<&str>, assistant_part_id: &str) -> bool {
    raw.and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .and_then(|value| {
            value
                .get("assistantPartId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|part| part == assistant_part_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ThreadEvent, ThreadEventRecord, ThreadSettings, ThreadSource, Turn, TurnStatus,
    };

    const THREAD: &str = "thread-1";
    const PART: &str = "assistant-part-1";

    fn record(sequence: u64, event: ThreadEvent) -> ThreadEventRecord {
        ThreadEventRecord {
            schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
            thread_id: THREAD.to_owned(),
            sequence,
            recorded_at_ms: 1_000 + sequence,
            event,
        }
    }

    fn started() -> ThreadEventRecord {
        record(
            1,
            ThreadEvent::ThreadStarted {
                session_root_id: THREAD.to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: "/w".to_owned(),
                    provider: "provider-a".to_owned(),
                    model: "model-a".to_owned(),
                    selected_skill_name: None,
                },
            },
        )
    }

    fn completed(notice: Option<&str>) -> ThreadEventRecord {
        let metadata = serde_json::json!({
            "assistantPartId": PART,
            "modelName": "model-a",
            "reasoningEffort": "max",
            "reflectionNotice": notice,
        });
        record(
            2,
            ThreadEvent::TurnCompleted {
                turn: Turn {
                    turn_id: "turn-1".to_owned(),
                    sequence: 0,
                    status: TurnStatus::Completed,
                    history_items: Vec::new(),
                    token_usage_json: Some(metadata.to_string()),
                    started_at_ms: 1_000,
                    completed_at_ms: Some(2_000),
                    interrupted_at_ms: None,
                    failure: None,
                },
            },
        )
    }

    #[test]
    fn metadata_patch_applies_to_the_matching_turn_only() {
        let patched = record(
            3,
            ThreadEvent::TurnMetadataPatched {
                assistant_part_id: PART.to_owned(),
                notice: Some("记忆反思：本轮新增 1 条记忆".to_owned()),
                affect_sequence: vec!["calm".to_owned()],
            },
        );
        let other = record(
            4,
            ThreadEvent::TurnMetadataPatched {
                assistant_part_id: "assistant-part-other".to_owned(),
                notice: Some("不该生效".to_owned()),
                affect_sequence: Vec::new(),
            },
        );
        let detail = &project_threads(&[started(), completed(None), patched, other])[0];
        let raw = detail.turns[0]
            .token_usage_json
            .as_deref()
            .expect("轮次必须还带着 metadata");
        let value: serde_json::Value = serde_json::from_str(raw).expect("metadata 必须是 JSON");
        assert_eq!(
            value["reflectionNotice"], "记忆反思：本轮新增 1 条记忆",
            "同一条轮的增量必须落地"
        );
        assert_eq!(value["affectSequence"], serde_json::json!(["calm"]));
        assert_eq!(value["modelName"], "model-a", "其余字段原样保留");
        assert_eq!(
            detail.turns[0].status,
            TurnStatus::Completed,
            "增量不得改写轮次状态"
        );
    }
}
