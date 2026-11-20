//! `run_thread_store` 的测试夹具：把一串 SQLite 轮次记录拼成 `ThreadDetail`。

use super::*;

/// 从轮次记录拼一份 thread 视图，只给测试用来核对模型历史装配。
pub(crate) fn thread_detail_from_records(
    records: &[RunSessionRecord],
) -> Result<ThreadDetail, String> {
    let fallback = records
        .last()
        .ok_or_else(|| "SQLite thread turns 不能为空".to_owned())?;
    let turns = records
        .iter()
        .cloned()
        .enumerate()
        .map(|(sequence, record)| {
            u64::try_from(sequence)
                .map_err(|_| "SQLite thread turn 数过大".to_owned())
                .and_then(|sequence| turn_from_record(&record, sequence))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ThreadDetail {
        thread: Thread {
            thread_id: fallback.thread_id.clone(),
            session_root_id: fallback.thread_id.clone(),
            parent_thread_id: fallback.parent_thread_id.clone(),
            forked_from_thread_id: None,
            name: None,
            status: ThreadStatus::Idle,
            source: ThreadSource::CliRun,
            ephemeral: false,
            settings: settings_from_record(fallback),
            child_origin: None,
            created_at_ms: fallback.created_at_ms,
            updated_at_ms: fallback.created_at_ms,
            archived_at_ms: None,
        },
        turns,
        compacted_history: None,
    })
}
