//! Memory 语义替代关系与原子归档。

use std::collections::BTreeSet;

use axon_domain::{AxonError, AxonResult, MemoryId, MemoryState};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};

use crate::{
    MemorySqliteRecord, MemorySqliteStore,
    store::{i64_from_u64, row_to_record, u64_from_i64},
};

/// 语义替代归档原因。
pub const MEMORY_SUPERSESSION_REASON: &str = "memory_semantic_superseded";

const CREATE_SUPERSESSION_SCHEMA_SQL: &str = r"
    CREATE TABLE IF NOT EXISTS memory_supersessions (
        predecessor_memory_id INTEGER PRIMARY KEY NOT NULL,
        successor_memory_id INTEGER NOT NULL,
        created_at_ms INTEGER NOT NULL,
        FOREIGN KEY(predecessor_memory_id) REFERENCES memory_records(memory_id) ON DELETE CASCADE,
        FOREIGN KEY(successor_memory_id) REFERENCES memory_records(memory_id) ON DELETE CASCADE,
        CHECK(predecessor_memory_id <> successor_memory_id)
    );
    CREATE INDEX IF NOT EXISTS idx_memory_supersessions_successor
        ON memory_supersessions(successor_memory_id, predecessor_memory_id);
    ";

/// 一条可审计的 Memory 语义替代边。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySupersession {
    /// 被替代的旧 Memory。
    pub predecessor_memory_id: MemoryId,
    /// 当前有效的新 Memory。
    pub successor_memory_id: MemoryId,
    /// 关系建立时间。
    pub created_at_ms: u64,
}

pub(crate) fn initialize_supersession_schema(connection: &Connection) -> AxonResult<()> {
    connection
        .execute_batch(CREATE_SUPERSESSION_SCHEMA_SQL)
        .map_err(|_| AxonError::memory_persistence("Memory 语义替代 schema 初始化失败"))
}

impl MemorySqliteStore {
    /// 原子写入 successor，并归档同作用域内被其明确替代的 active Memory。
    pub fn upsert_record_with_supersessions(
        &self,
        successor: &MemorySqliteRecord,
        predecessor_ids: &[MemoryId],
        now_ms: u64,
    ) -> AxonResult<Vec<MemorySupersession>> {
        let predecessor_ids = validate_request(successor, predecessor_ids, now_ms)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| AxonError::memory_persistence("Memory 语义替代事务启动失败"))?;
        let predecessors = load_predecessors(&transaction, &predecessor_ids)?;
        validate_predecessors(&transaction, successor, &predecessors)?;
        crate::store_write::upsert_record_on_connection(&transaction, successor)?;

        let relations = predecessors
            .iter()
            .map(|predecessor| {
                persist_relation(&transaction, predecessor, successor.record.id, now_ms)
            })
            .collect::<AxonResult<Vec<_>>>()?;
        transaction
            .commit()
            .map_err(|_| AxonError::memory_persistence("Memory 语义替代事务提交失败"))?;
        Ok(relations)
    }

    /// 按 successor 查询其直接替代的 Memory。
    pub fn supersessions_by_successor(
        &self,
        successor_memory_id: MemoryId,
    ) -> AxonResult<Vec<MemorySupersession>> {
        if successor_memory_id.0 == 0 {
            return Err(AxonError::memory_persistence("Memory successor ID 无效"));
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT predecessor_memory_id, successor_memory_id, created_at_ms \
                 FROM memory_supersessions WHERE successor_memory_id = ?1 \
                 ORDER BY predecessor_memory_id ASC",
            )
            .map_err(|_| AxonError::memory_persistence("Memory 语义替代关系查询失败"))?;
        statement
            .query_map([i64_from_u64(successor_memory_id.0)?], |row| {
                Ok(MemorySupersession {
                    predecessor_memory_id: MemoryId(u64_from_i64(row.get(0)?)?),
                    successor_memory_id: MemoryId(u64_from_i64(row.get(1)?)?),
                    created_at_ms: u64_from_i64(row.get(2)?)?,
                })
            })
            .map_err(|_| AxonError::memory_persistence("Memory 语义替代关系读取失败"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AxonError::memory_persistence("Memory 语义替代关系解析失败"))
    }
}

fn validate_request(
    successor: &MemorySqliteRecord,
    predecessor_ids: &[MemoryId],
    now_ms: u64,
) -> AxonResult<Vec<MemoryId>> {
    crate::store::validate_record(successor)?;
    if now_ms == 0
        || successor.archived_at_ms.is_some()
        || !matches!(
            successor.record.state,
            MemoryState::Weak | MemoryState::Strong
        )
        || predecessor_ids.is_empty()
    {
        return Err(AxonError::memory_persistence("Memory 语义替代输入无效"));
    }
    let unique = predecessor_ids.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != predecessor_ids.len()
        || unique.contains(&MemoryId(0))
        || unique.contains(&successor.record.id)
    {
        return Err(AxonError::memory_persistence(
            "Memory 语义替代 ID 无效或重复",
        ));
    }
    Ok(unique.into_iter().collect())
}

fn load_predecessors(
    transaction: &Transaction<'_>,
    predecessor_ids: &[MemoryId],
) -> AxonResult<Vec<MemorySqliteRecord>> {
    let mut statement = transaction
        .prepare("SELECT * FROM memory_records WHERE memory_id = ?1")
        .map_err(|_| AxonError::memory_persistence("Memory 语义替代目标查询失败"))?;
    predecessor_ids
        .iter()
        .map(|memory_id| {
            statement
                .query_row([i64_from_u64(memory_id.0)?], row_to_record)
                .optional()
                .map_err(|_| AxonError::memory_persistence("Memory 语义替代目标读取失败"))?
                .ok_or_else(|| AxonError::memory_persistence("Memory 语义替代目标不存在"))
        })
        .collect()
}

fn validate_predecessors(
    transaction: &Transaction<'_>,
    successor: &MemorySqliteRecord,
    predecessors: &[MemorySqliteRecord],
) -> AxonResult<()> {
    for predecessor in predecessors {
        if predecessor.namespace_ref != successor.namespace_ref
            || predecessor.repo_ref != successor.repo_ref
            || predecessor.user_ref != successor.user_ref
        {
            return Err(AxonError::memory_persistence(
                "Memory 语义替代不能跨越作用域",
            ));
        }
        let existing_successor = existing_successor(transaction, predecessor.record.id)?;
        let active = predecessor.archived_at_ms.is_none()
            && matches!(
                predecessor.record.state,
                MemoryState::Weak | MemoryState::Strong
            );
        let idempotent = predecessor.archived_at_ms.is_some()
            && predecessor
                .record
                .tombstone
                .as_ref()
                .is_some_and(|tombstone| tombstone.reason == MEMORY_SUPERSESSION_REASON)
            && existing_successor == Some(successor.record.id);
        if !active && !idempotent {
            return Err(AxonError::memory_persistence(
                "Memory 语义替代目标不是可替代的 active 记录",
            ));
        }
        if active && existing_successor.is_some() {
            return Err(AxonError::memory_persistence("Memory 语义替代关系已经存在"));
        }
    }
    Ok(())
}

fn existing_successor(
    transaction: &Transaction<'_>,
    predecessor_memory_id: MemoryId,
) -> AxonResult<Option<MemoryId>> {
    transaction
        .query_row(
            "SELECT successor_memory_id FROM memory_supersessions WHERE predecessor_memory_id = ?1",
            [i64_from_u64(predecessor_memory_id.0)?],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|_| AxonError::memory_persistence("Memory 语义替代关系读取失败"))?
        .map(|value| {
            value
                .try_into()
                .map(MemoryId)
                .map_err(|_| AxonError::memory_persistence("Memory 语义替代关系 ID 无效"))
        })
        .transpose()
}

fn persist_relation(
    transaction: &Transaction<'_>,
    predecessor: &MemorySqliteRecord,
    successor_memory_id: MemoryId,
    now_ms: u64,
) -> AxonResult<MemorySupersession> {
    let relation = MemorySupersession {
        predecessor_memory_id: predecessor.record.id,
        successor_memory_id,
        created_at_ms: now_ms,
    };
    if predecessor.archived_at_ms.is_some() {
        return Ok(relation);
    }
    let blocked_ref = predecessor
        .record
        .content_ref
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AxonError::memory_persistence("Memory 语义替代目标缺少 content ref"))?;
    transaction
        .execute(
            "UPDATE memory_records SET tombstone_blocked_ref = ?2, tombstone_reason = ?3, \
             updated_at_ms = ?4, archived_at_ms = ?4 WHERE memory_id = ?1",
            params![
                i64_from_u64(predecessor.record.id.0)?,
                blocked_ref,
                MEMORY_SUPERSESSION_REASON,
                i64_from_u64(now_ms)?,
            ],
        )
        .map_err(|_| AxonError::memory_persistence("Memory 语义替代目标归档失败"))?;
    transaction
        .execute(
            "INSERT INTO memory_supersessions (predecessor_memory_id, successor_memory_id, created_at_ms) \
             VALUES (?1, ?2, ?3)",
            params![
                i64_from_u64(predecessor.record.id.0)?,
                i64_from_u64(successor_memory_id.0)?,
                i64_from_u64(now_ms)?,
            ],
        )
        .map_err(|_| AxonError::memory_persistence("Memory 语义替代关系写入失败"))?;
    Ok(relation)
}
