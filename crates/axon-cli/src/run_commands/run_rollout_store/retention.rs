//! rollout 会话保留策略：过期会话家族识别、原子删除与空间回收。
//!
//! 保留策略以「会话家族」为单位：沿 parent_session_id 归并到根，只要家族里有一条
//! 受保护会话或未过期会话，整个家族都不清理，避免回放链被删成断头。

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension as _};

use super::{CURRENT_SESSION_SINGLETON_KEY, RolloutSqliteStore};

/// 每轮会话清理最多回收的 SQLite 空闲页数。
const ROLLOUT_INCREMENTAL_VACUUM_PAGE_BATCH: u32 = 256;
const ROLLOUT_CLEANUP_BATCH_LIMIT: usize = 256;

/// rollout 会话清理结果。
#[derive(Debug, Default)]
pub(crate) struct RolloutCleanupReport {
    /// 已删除的会话 ID。
    pub(crate) session_ids: Vec<String>,
    /// 是否完成空间回收。
    pub(crate) vacuumed: bool,
}

#[derive(Debug)]
struct RolloutSessionMetadata {
    session_id: String,
    parent_session_id: Option<String>,
    updated_at_ms: i64,
}

impl RolloutSqliteStore {
    /// 只读查找已过期且不属于当前会话树的 rollout 会话家族。
    pub(crate) fn stale_session_family_ids(
        &mut self,
        cutoff_ms: u64,
        protected_session_ids: &HashSet<String>,
    ) -> Result<Vec<String>, String> {
        let cutoff_ms = i64::try_from(cutoff_ms).unwrap_or(i64::MAX);
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| "rollout 清理查询事务启动失败".to_owned())?;
        let stale_ids =
            stale_session_ids_in_transaction(&transaction, cutoff_ms, protected_session_ids)?;
        transaction
            .commit()
            .map_err(|_| "rollout 清理查询事务提交失败".to_owned())?;
        Ok(stale_ids)
    }

    /// 在附属存储清理完成后，重新校验并原子删除仍过期的候选会话家族。
    pub(crate) fn finalize_stale_session_family_removal(
        &mut self,
        candidate_session_ids: &[String],
        cutoff_ms: u64,
        protected_session_ids: &HashSet<String>,
    ) -> Result<RolloutCleanupReport, String> {
        if candidate_session_ids.is_empty() {
            return Ok(RolloutCleanupReport::default());
        }
        let cutoff_ms = i64::try_from(cutoff_ms).unwrap_or(i64::MAX);
        let candidates = candidate_session_ids.iter().collect::<HashSet<_>>();
        let mut transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "rollout 清理事务启动失败".to_owned())?;
        let stale_ids =
            stale_session_ids_in_transaction(&transaction, cutoff_ms, protected_session_ids)?
                .into_iter()
                .filter(|session_id| candidates.contains(session_id))
                .collect::<Vec<_>>();
        if stale_ids.is_empty() {
            transaction
                .commit()
                .map_err(|_| "rollout 空清理事务提交失败".to_owned())?;
            return Ok(RolloutCleanupReport::default());
        }
        let first_batch_len = stale_ids.len().min(ROLLOUT_CLEANUP_BATCH_LIMIT);
        delete_rollout_session_rows(&transaction, &stale_ids[..first_batch_len])?;
        transaction
            .commit()
            .map_err(|_| "rollout 会话清理事务提交失败".to_owned())?;
        for ids in stale_ids[first_batch_len..].chunks(ROLLOUT_CLEANUP_BATCH_LIMIT) {
            transaction = self
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|_| "rollout 清理事务启动失败".to_owned())?;
            delete_rollout_session_rows(&transaction, ids)?;
            transaction
                .commit()
                .map_err(|_| "rollout 会话清理事务提交失败".to_owned())?;
        }
        reclaim_rollout_space(&self.connection)?;
        Ok(RolloutCleanupReport {
            session_ids: stale_ids,
            vacuumed: true,
        })
    }
}

fn session_root(session_id: &str, parents: &HashMap<String, Option<String>>) -> String {
    let mut current = session_id.to_owned();
    let mut visited = HashSet::new();
    while let Some(Some(parent)) = parents.get(&current) {
        if !visited.insert(current.clone()) || !parents.contains_key(parent) {
            break;
        }
        current.clone_from(parent);
    }
    current
}

fn read_rollout_session_metadata(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Vec<RolloutSessionMetadata>, String> {
    let mut statement = transaction
        .prepare("SELECT session_id, parent_session_id, updated_at_ms FROM rollout_sessions")
        .map_err(|_| "rollout 会话清理查询失败".to_owned())?;
    let rows = statement
        .query_map([], |row| {
            Ok(RolloutSessionMetadata {
                session_id: row.get(0)?,
                parent_session_id: row.get(1)?,
                updated_at_ms: row.get(2)?,
            })
        })
        .map_err(|_| "rollout 会话清理查询失败".to_owned())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| "rollout 会话清理记录读取失败".to_owned())
}

fn stale_session_ids_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    cutoff_ms: i64,
    protected_session_ids: &HashSet<String>,
) -> Result<Vec<String>, String> {
    let current_session_id = transaction
        .query_row(
            "SELECT session_id FROM rollout_current_session WHERE singleton_key = ?1",
            [CURRENT_SESSION_SINGLETON_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| "rollout 当前会话读取失败".to_owned())?;
    let sessions = read_rollout_session_metadata(transaction)?;
    Ok(stale_session_ids(
        sessions,
        current_session_id.as_deref(),
        protected_session_ids,
        cutoff_ms,
    ))
}

fn stale_session_ids(
    sessions: Vec<RolloutSessionMetadata>,
    current_session_id: Option<&str>,
    protected_session_ids: &HashSet<String>,
    cutoff_ms: i64,
) -> Vec<String> {
    let parents = sessions
        .iter()
        .map(|session| {
            (
                session.session_id.clone(),
                session.parent_session_id.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let protected_root = current_session_id.map(|session_id| session_root(session_id, &parents));
    let mut families = HashMap::<String, Vec<RolloutSessionMetadata>>::new();
    for session in sessions {
        let root = session_root(&session.session_id, &parents);
        families.entry(root).or_default().push(session);
    }
    let mut stale_ids = families
        .into_iter()
        .filter(|(root, members)| {
            protected_root.as_deref() != Some(root.as_str())
                && !members
                    .iter()
                    .any(|session| protected_session_ids.contains(&session.session_id))
                && members
                    .iter()
                    .all(|session| session.updated_at_ms < cutoff_ms)
        })
        .flat_map(|(_, members)| members.into_iter().map(|session| session.session_id))
        .collect::<Vec<_>>();
    stale_ids.sort();
    stale_ids
}

fn delete_rollout_session_rows(
    transaction: &rusqlite::Transaction<'_>,
    session_ids: &[String],
) -> Result<(), String> {
    for session_id in session_ids {
        for (table, message) in [
            ("rollout_events", "rollout 事件清理失败"),
            ("rollout_turns", "rollout turn 清理失败"),
            ("rollout_sessions", "rollout 会话清理失败"),
        ] {
            transaction
                .execute(
                    &format!("DELETE FROM {table} WHERE session_id = ?1"),
                    [session_id],
                )
                .map_err(|_| message.to_owned())?;
        }
    }
    Ok(())
}

fn reclaim_rollout_space(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(&format!(
            "PRAGMA incremental_vacuum({ROLLOUT_INCREMENTAL_VACUUM_PAGE_BATCH})"
        ))
        .map_err(|_| "rollout SQLite 增量空间回收失败".to_owned())
}
