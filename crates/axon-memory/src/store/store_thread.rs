//! 承诺（commitments）与单前台线索（threads）的 SQLite schema 与最小读写。
//!
//! M3-1 只负责建表与最小 CRUD：旧库升级只追加表与索引，不删除任何 `memory_records` 行。
//! 线索策略（何时新建、何时沿用）在 `crate::thread` 里实现，本文件不判断语义。

use rusqlite::{Connection, OptionalExtension, params};

use axon_domain::{AxonError, AxonResult};

use crate::MemorySqliteStore;

/// 承诺处于待办状态。
pub const COMMITMENT_STATUS_OPEN: &str = "open";
/// 承诺已关闭（用户表示完成或明确不再需要）。
pub const COMMITMENT_STATUS_DONE: &str = "done";
/// 线索处于前台活跃状态。
pub const THREAD_STATUS_OPEN: &str = "open";
/// 线索已归档，不再作为前台线索复用。
pub const THREAD_STATUS_ARCHIVED: &str = "archived";

/// 一条承诺记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCommitment {
    /// 承诺 ID。
    pub commitment_id: String,
    /// 归属线索；未归属线索时为 `None`。
    pub thread_id: Option<String>,
    /// 命名空间引用。
    pub namespace_ref: String,
    /// owner principal。
    pub user_ref: String,
    /// 承诺原文。
    pub text: String,
    /// 状态：`open` / `done`。
    pub status: String,
    /// 来源通道；未知时为空串。
    pub channel: String,
    /// 创建时间。
    pub created_at_ms: i64,
    /// 关闭时间。
    pub closed_at_ms: Option<i64>,
}

/// 一条前台线索记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryThread {
    /// 线索 ID。
    pub thread_id: String,
    /// 命名空间引用。
    pub namespace_ref: String,
    /// owner principal。
    pub user_ref: String,
    /// 线索主题（用于 P2 渲染的兜底文本）。
    pub topic: String,
    /// 线索签名（预留给语义去重，本期只写入不参与判断）。
    pub signature: String,
    /// 展示标签。
    pub label: String,
    /// 摘要。
    pub summary: String,
    /// 状态：`open` / `archived`。
    pub status: String,
    /// 命中次数（每轮沿用前台线索时 +1）。
    pub hit_count: i64,
    /// 最近事件时间。
    pub last_event_at_ms: i64,
    /// 创建时间。
    pub created_at_ms: i64,
    /// 更新时间。
    pub updated_at_ms: i64,
}

/// 建立承诺与线索表；`CREATE TABLE IF NOT EXISTS` 保证可重复执行。
pub(crate) fn initialize_thread_schema(connection: &Connection) -> AxonResult<()> {
    connection
        .execute_batch(
            r"
            CREATE TABLE IF NOT EXISTS memory_commitments (
                commitment_id TEXT PRIMARY KEY,
                thread_id     TEXT,
                namespace_ref TEXT NOT NULL,
                user_ref      TEXT NOT NULL,
                text          TEXT NOT NULL,
                status        TEXT NOT NULL DEFAULT 'open',
                channel       TEXT NOT NULL DEFAULT '',
                created_at_ms INTEGER NOT NULL,
                closed_at_ms  INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_commit_open
                ON memory_commitments(namespace_ref, status, created_at_ms DESC);

            CREATE TABLE IF NOT EXISTS memory_threads (
                thread_id        TEXT PRIMARY KEY,
                namespace_ref    TEXT NOT NULL,
                user_ref         TEXT NOT NULL,
                topic            TEXT NOT NULL,
                signature        TEXT NOT NULL,
                label            TEXT NOT NULL DEFAULT '',
                summary          TEXT NOT NULL DEFAULT '',
                status           TEXT NOT NULL DEFAULT 'open',
                hit_count        INTEGER NOT NULL DEFAULT 1,
                last_event_at_ms INTEGER NOT NULL,
                created_at_ms    INTEGER NOT NULL,
                updated_at_ms    INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_threads_fg
                ON memory_threads(namespace_ref, status, last_event_at_ms DESC);
            ",
        )
        .map_err(|_| AxonError::memory_persistence("Memory 承诺/线索 schema 初始化失败"))
}

impl MemorySqliteStore {
    /// 写入一条线索；同 ID 已存在时覆盖可变字段。
    pub fn upsert_thread(&self, thread: &MemoryThread) -> AxonResult<()> {
        self.connection
            .execute(
                r"
                INSERT INTO memory_threads (
                    thread_id, namespace_ref, user_ref, topic, signature, label, summary,
                    status, hit_count, last_event_at_ms, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(thread_id) DO UPDATE SET
                    topic = excluded.topic,
                    label = excluded.label,
                    summary = excluded.summary,
                    status = excluded.status,
                    hit_count = excluded.hit_count,
                    last_event_at_ms = excluded.last_event_at_ms,
                    updated_at_ms = excluded.updated_at_ms
                ",
                params![
                    thread.thread_id,
                    thread.namespace_ref,
                    thread.user_ref,
                    thread.topic,
                    thread.signature,
                    thread.label,
                    thread.summary,
                    thread.status,
                    thread.hit_count,
                    thread.last_event_at_ms,
                    thread.created_at_ms,
                    thread.updated_at_ms,
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 线索写入失败"))?;
        Ok(())
    }

    /// 取命名空间内最近活跃的前台线索（`status = open` 且 `last_event_at_ms` 最新）。
    pub fn foreground_thread(
        &self,
        namespace_ref: &str,
        user_ref: &str,
    ) -> AxonResult<Option<MemoryThread>> {
        self.connection
            .query_row(
                &format!(
                    "SELECT {THREAD_COLUMNS} FROM memory_threads \
                     WHERE namespace_ref = ?1 AND user_ref = ?2 AND status = ?3 \
                     ORDER BY last_event_at_ms DESC, thread_id ASC LIMIT 1"
                ),
                params![namespace_ref, user_ref, THREAD_STATUS_OPEN],
                row_to_thread,
            )
            .optional()
            .map_err(|_| AxonError::memory_persistence("Memory 前台线索读取失败"))
    }

    /// 前台线索沿用一次：`hit_count += 1` 并刷新活跃时间。
    pub fn touch_thread(&self, thread_id: &str, now_ms: i64) -> AxonResult<bool> {
        let changed = self
            .connection
            .execute(
                "UPDATE memory_threads SET hit_count = hit_count + 1, \
                 last_event_at_ms = ?2, updated_at_ms = ?2 WHERE thread_id = ?1",
                params![thread_id, now_ms],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 线索活跃时间更新失败"))?;
        Ok(changed > 0)
    }

    /// 归档线索；归档后不再作为前台线索复用。
    pub fn archive_thread(&self, thread_id: &str, now_ms: i64) -> AxonResult<bool> {
        let changed = self
            .connection
            .execute(
                "UPDATE memory_threads SET status = ?2, updated_at_ms = ?3 WHERE thread_id = ?1",
                params![thread_id, THREAD_STATUS_ARCHIVED, now_ms],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 线索归档失败"))?;
        Ok(changed > 0)
    }

    /// 写入或更新承诺。
    ///
    /// 单例语义：同一线索已有 `open` 承诺时只更新原文与时间，不新增一行。
    pub fn upsert_open_commitment(&self, commitment: &MemoryCommitment) -> AxonResult<()> {
        if let Some(thread_id) = commitment.thread_id.as_deref() {
            let existing: Option<String> = self
                .connection
                .query_row(
                    "SELECT commitment_id FROM memory_commitments \
                     WHERE thread_id = ?1 AND status = ?2 ORDER BY created_at_ms DESC, commitment_id ASC LIMIT 1",
                    params![thread_id, COMMITMENT_STATUS_OPEN],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| AxonError::memory_persistence("Memory 承诺读取失败"))?;
            if let Some(existing_id) = existing {
                self.connection
                    .execute(
                        "UPDATE memory_commitments SET text = ?2, namespace_ref = ?3, user_ref = ?4, \
                         channel = ?5 WHERE commitment_id = ?1",
                        params![
                            existing_id,
                            commitment.text,
                            commitment.namespace_ref,
                            commitment.user_ref,
                            commitment.channel,
                        ],
                    )
                    .map_err(|_| AxonError::memory_persistence("Memory 承诺更新失败"))?;
                return Ok(());
            }
        }
        self.connection
            .execute(
                r"
                INSERT INTO memory_commitments (
                    commitment_id, thread_id, namespace_ref, user_ref, text, status, channel,
                    created_at_ms, closed_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ",
                params![
                    commitment.commitment_id,
                    commitment.thread_id,
                    commitment.namespace_ref,
                    commitment.user_ref,
                    commitment.text,
                    commitment.status,
                    commitment.channel,
                    commitment.created_at_ms,
                    commitment.closed_at_ms,
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 承诺写入失败"))?;
        Ok(())
    }

    /// 关闭承诺：置为目标状态并记录关闭时间。
    pub fn close_commitment(
        &self,
        commitment_id: &str,
        status: &str,
        now_ms: i64,
    ) -> AxonResult<bool> {
        let changed = self
            .connection
            .execute(
                "UPDATE memory_commitments SET status = ?2, closed_at_ms = ?3 \
                 WHERE commitment_id = ?1 AND status = ?4",
                params![commitment_id, status, now_ms, COMMITMENT_STATUS_OPEN],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 承诺关闭失败"))?;
        Ok(changed > 0)
    }

    /// 按 `created_at_ms DESC, commitment_id ASC` 取未完成承诺。
    pub fn open_commitments(
        &self,
        namespace_ref: &str,
        user_ref: &str,
        limit: usize,
    ) -> AxonResult<Vec<MemoryCommitment>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {COMMITMENT_COLUMNS} FROM memory_commitments \
                 WHERE namespace_ref = ?1 AND user_ref = ?2 AND status = ?3 \
                 ORDER BY created_at_ms DESC, commitment_id ASC LIMIT ?4"
            ))
            .map_err(|_| AxonError::memory_persistence("Memory 承诺列表读取失败"))?;
        let rows = statement
            .query_map(
                params![namespace_ref, user_ref, COMMITMENT_STATUS_OPEN, limit],
                row_to_commitment,
            )
            .map_err(|_| AxonError::memory_persistence("Memory 承诺列表读取失败"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| AxonError::memory_persistence("Memory 承诺列表读取失败"))
    }

    /// 当前库内承诺总数（供迁移与验收断言使用）。
    pub fn commitment_count(&self) -> AxonResult<usize> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM memory_commitments", [], |row| {
                row.get(0)
            })
            .map_err(|_| AxonError::memory_persistence("Memory 承诺计数失败"))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// 当前库内线索总数（供迁移与验收断言使用）。
    pub fn thread_count(&self) -> AxonResult<usize> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM memory_threads", [], |row| row.get(0))
            .map_err(|_| AxonError::memory_persistence("Memory 线索计数失败"))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }
}

const THREAD_COLUMNS: &str = "thread_id, namespace_ref, user_ref, topic, signature, label, summary, \
     status, hit_count, last_event_at_ms, created_at_ms, updated_at_ms";

const COMMITMENT_COLUMNS: &str = "commitment_id, thread_id, namespace_ref, user_ref, text, status, \
     channel, created_at_ms, closed_at_ms";

fn row_to_thread(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryThread> {
    Ok(MemoryThread {
        thread_id: row.get(0)?,
        namespace_ref: row.get(1)?,
        user_ref: row.get(2)?,
        topic: row.get(3)?,
        signature: row.get(4)?,
        label: row.get(5)?,
        summary: row.get(6)?,
        status: row.get(7)?,
        hit_count: row.get(8)?,
        last_event_at_ms: row.get(9)?,
        created_at_ms: row.get(10)?,
        updated_at_ms: row.get(11)?,
    })
}

fn row_to_commitment(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryCommitment> {
    Ok(MemoryCommitment {
        commitment_id: row.get(0)?,
        thread_id: row.get(1)?,
        namespace_ref: row.get(2)?,
        user_ref: row.get(3)?,
        text: row.get(4)?,
        status: row.get(5)?,
        channel: row.get(6)?,
        created_at_ms: row.get(7)?,
        closed_at_ms: row.get(8)?,
    })
}

#[cfg(test)]
#[path = "store_thread_tests.rs"]
mod tests;
