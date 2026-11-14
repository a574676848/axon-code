//! Memory 治理记录的原子写入面：治理更新、删除、归档与撤销归档。
//!
//! 每个操作都在单个 SQLite 事务内完成，调用方只关心「影响了几行」，不承担事务细节。

use axon_domain::{AxonError, AxonResult, MemoryId};
use rusqlite::{Connection, params};

use super::{
    MemorySqliteRecord, i64_from_u64, optional_i64_from_u64, state_code, unique_positive_i64s,
    validate_record,
};

/// 原子更新一组既有记录的治理状态与排序元数据。
pub(super) fn update_governance_records(
    connection: &Connection,
    records: &[MemorySqliteRecord],
) -> AxonResult<()> {
    if records.is_empty() {
        return Ok(());
    }
    for record in records {
        validate_record(record)?;
    }
    let transaction = connection
        .unchecked_transaction()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 治理事务启动失败"))?;
    for record in records {
        transaction
            .execute(
                r"
                UPDATE memory_records SET
                    state = ?2,
                    content_ref = ?3,
                    tombstone_blocked_ref = ?4,
                    tombstone_reason = ?5,
                    updated_at_ms = ?6,
                    last_active_at_ms = ?7,
                    confidence_basis_points = ?8,
                    importance_basis_points = ?9,
                    archived_at_ms = ?10
                WHERE memory_id = ?1
                ",
                params![
                    i64_from_u64(record.record.id.0)?,
                    state_code(record.record.state),
                    record.record.content_ref.as_deref(),
                    record
                        .record
                        .tombstone
                        .as_ref()
                        .map(|item| item.blocked_ref.as_str()),
                    record
                        .record
                        .tombstone
                        .as_ref()
                        .map(|item| item.reason.as_str()),
                    i64_from_u64(record.updated_at_ms)?,
                    i64_from_u64(record.last_active_at_ms)?,
                    i64::from(record.confidence_basis_points),
                    i64::from(record.importance_basis_points),
                    optional_i64_from_u64(record.archived_at_ms)?,
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 治理记录更新失败"))?;
    }
    transaction
        .commit()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 治理事务提交失败"))
}

/// 原子删除一组过期 Memory 记录。
pub(super) fn delete_records(
    connection: &Connection,
    memory_ids: &[MemoryId],
) -> AxonResult<usize> {
    let values = unique_positive_i64s(memory_ids.iter().map(|id| id.0))?;
    if values.is_empty() {
        return Ok(0);
    }
    let transaction = connection
        .unchecked_transaction()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 删除事务启动失败"))?;
    let mut deleted = 0usize;
    for memory_id in values {
        deleted = deleted.saturating_add(
            transaction
                .execute(
                    "DELETE FROM memory_records WHERE memory_id = ?1",
                    [memory_id],
                )
                .map_err(|_| AxonError::memory_persistence("Memory SQLite 记录删除失败"))?,
        );
    }
    transaction
        .commit()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 删除事务提交失败"))?;
    Ok(deleted)
}

/// 原子归档一组 Memory：只写 `archived_at_ms`，不动内容与召回状态。
///
/// 与淘汰不同：淘汰写 tombstone 并建立 supersession 链，归档只是让记录退出召回，
/// 事后仍能按 archived 过滤回看，属可逆动作。
pub(super) fn archive_records(
    connection: &Connection,
    memory_ids: &[MemoryId],
    now_ms: u64,
) -> AxonResult<usize> {
    if now_ms == 0 {
        return Err(AxonError::memory_persistence("Memory 归档时间无效"));
    }
    let values = unique_positive_i64s(memory_ids.iter().map(|id| id.0))?;
    if values.is_empty() {
        return Ok(0);
    }
    let archived_at_ms = i64_from_u64(now_ms)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 归档事务启动失败"))?;
    let mut archived = 0usize;
    for memory_id in values {
        archived = archived.saturating_add(
            transaction
                .execute(
                    "UPDATE memory_records SET archived_at_ms = ?2, updated_at_ms = ?2 \
                     WHERE memory_id = ?1 AND archived_at_ms IS NULL \
                       AND LOWER(state) <> 'tombstone'",
                    [memory_id, archived_at_ms],
                )
                .map_err(|_| AxonError::memory_persistence("Memory SQLite 归档更新失败"))?,
        );
    }
    transaction
        .commit()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 归档事务提交失败"))?;
    Ok(archived)
}

/// 撤销归档：只清 `archived_at_ms`，不动正文与状态机。
///
/// 这是归档「可逆」的实现面：归档只是让记录退出召回，撤销后按原状态回到召回集合。
/// 已被淘汰（tombstone）的记录不会因为撤销归档复活，这里同样按状态排除。
pub(super) fn unarchive_records(
    connection: &Connection,
    memory_ids: &[MemoryId],
    now_ms: u64,
) -> AxonResult<usize> {
    if now_ms == 0 {
        return Err(AxonError::memory_persistence("Memory 撤销归档时间无效"));
    }
    let values = unique_positive_i64s(memory_ids.iter().map(|id| id.0))?;
    if values.is_empty() {
        return Ok(0);
    }
    let now = i64_from_u64(now_ms)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 撤销归档事务启动失败"))?;
    let mut restored = 0usize;
    for memory_id in values {
        restored = restored.saturating_add(
            transaction
                .execute(
                    "UPDATE memory_records SET archived_at_ms = NULL, updated_at_ms = ?2 \
                     WHERE memory_id = ?1 AND archived_at_ms IS NOT NULL \
                       AND LOWER(state) <> 'tombstone'",
                    [memory_id, now],
                )
                .map_err(|_| AxonError::memory_persistence("Memory SQLite 撤销归档更新失败"))?,
        );
    }
    transaction
        .commit()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 撤销归档事务提交失败"))?;
    Ok(restored)
}
