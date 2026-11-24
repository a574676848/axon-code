//! 会话级上下文快照的读写。
//!
//! 表是投影：一行对应一条会话，只由事件追加的唯一投影入口（`apply_projections`）写入。
//! 写入失败只记日志、不阻断落盘（与 `session_metrics` 同款），否则投影缺陷会被放大成
//! 写入故障；下一次可用读数会覆盖修正。
//!
//! 读数的口径只有一处：用量帧里的 Provider 实测值。换窗不再写入任何前后读数，
//! 头部因此只展示「最近一次请求占了多少窗口」，剩余量由消费方按窗口与实测占用派生。

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage};
use crate::domain::ContextSnapshot;
use crate::ports::{ContextSnapshotStore, RuntimeStoreError};
use axon_domain::DaemonSessionId;

/// 用量帧写出的快照来源标记。
pub(super) const SOURCE_USAGE: &str = "usage_frame";

/// 一批事件里可用的上下文读数。
///
/// 只保留用量读数：换窗不再产生 token 读数，表里也就没有第二类槽位。
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct ContextSnapshotUpdate {
    /// 本批最后一次可用的用量读数。
    pub(super) usage: Option<UsageReading>,
}

impl ContextSnapshotUpdate {
    /// 这一批没有任何可用读数：整批跳过，不产生写入，也不刷新时间戳。
    pub(super) const fn is_empty(&self) -> bool {
        self.usage.is_none()
    }
}

/// 一次用量读数的可存储形状。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct UsageReading {
    /// 当前上下文占用 token（Provider 实测）。
    pub(super) current_context_tokens: u64,
    /// 帧里带有的上下文窗口。
    pub(super) context_window: Option<u64>,
    /// 帧里带有的上下文构成。
    pub(super) composition: Option<serde_json::Value>,
    /// 该帧的记录时刻。
    pub(super) observed_at_ms: u64,
}

/// 把一批读数合并进会话级快照。
///
/// 读写同一个事务：先读旧行再合并，最后就地更新。没有可用读数时调用方根本不会走到
/// 这里，因此「不倒退、不粘旧值」由调用方的过滤与这里的事务共同保证。
pub(super) fn apply_update(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    update: &ContextSnapshotUpdate,
) -> Result<(), RuntimeStoreError> {
    if update.is_empty() {
        return Ok(());
    }
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let mut row = read_row(&transaction, session_id)?.unwrap_or_default();
    if let Some(usage) = update.usage.as_ref() {
        row.current_context_tokens = Some(usage.current_context_tokens);
        row.context_window = usage.context_window;
        // 构成与占用是同一份读数：新的读数没有构成时留空，不拿旧构成拼一个新数字。
        row.composition = usage.composition.clone();
        // 时刻只前进不后退：迟到的旧帧可以更新读数，但不能把「更新于」拉回过去。
        row.updated_at_ms = Some(fresher(row.updated_at_ms, usage.observed_at_ms));
        row.source = Some(SOURCE_USAGE.to_owned());
    }
    write_row(&transaction, session_id, &row)?;
    transaction.commit().map_err(storage)
}

/// 两条时刻里较新的一条；缺失按「更早」处理。
fn fresher(current: Option<u64>, candidate: u64) -> u64 {
    current.map_or(candidate, |current| current.max(candidate))
}

/// 读取会话级上下文快照；没有记录时返回 `None`。
pub(super) fn read_snapshot(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
) -> Result<Option<ContextSnapshot>, RuntimeStoreError> {
    let connection = store.connection()?;
    Ok(read_row(&connection, session_id)?.map(snapshot_of))
}

impl ContextSnapshotStore for SqliteRuntimeStore {
    fn context_snapshot(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Option<ContextSnapshot>, RuntimeStoreError> {
        read_snapshot(self, session_id)
    }
}

/// 一行快照的列形状：读旧行、合并、写回共用这一份定义。
///
/// 库里的 `rollover_before_tokens` / `rollover_after_tokens` 是历史列，读取方一律不碰：
/// 换窗读数已整体下线，SELECT 与 INSERT 都不再引用它们。
#[derive(Debug, Clone, Default)]
struct SnapshotRow {
    current_context_tokens: Option<u64>,
    context_window: Option<u64>,
    composition: Option<serde_json::Value>,
    source: Option<String>,
    updated_at_ms: Option<u64>,
}

fn read_row(
    connection: &rusqlite::Connection,
    session_id: &DaemonSessionId,
) -> Result<Option<SnapshotRow>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT current_context_tokens,context_window,composition_json,source,updated_at_ms \
             FROM context_snapshots WHERE session_id=?1",
            params![session_id.as_str()],
            |row| {
                Ok(SnapshotRow {
                    current_context_tokens: optional_count(row.get(0)?),
                    context_window: optional_count(row.get(1)?),
                    composition: row
                        .get::<_, Option<String>>(2)?
                        .and_then(|raw| serde_json::from_str(&raw).ok()),
                    source: row.get(3)?,
                    updated_at_ms: optional_count(row.get(4)?),
                })
            },
        )
        .optional()
        .map_err(storage)
}

fn snapshot_of(row: SnapshotRow) -> ContextSnapshot {
    ContextSnapshot {
        current_context_tokens: row.current_context_tokens,
        context_window: row.context_window,
        composition: row.composition,
        updated_at_ms: row.updated_at_ms,
        source: row.source,
    }
}

fn write_row(
    connection: &rusqlite::Connection,
    session_id: &DaemonSessionId,
    row: &SnapshotRow,
) -> Result<(), RuntimeStoreError> {
    connection
        .execute(
            "INSERT INTO context_snapshots(
                 session_id,current_context_tokens,context_window,composition_json,\
                 source,updated_at_ms
             ) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(session_id) DO UPDATE SET
                 current_context_tokens=excluded.current_context_tokens,
                 context_window=excluded.context_window,
                 composition_json=excluded.composition_json,
                 source=excluded.source,
                 updated_at_ms=excluded.updated_at_ms",
            params![
                session_id.as_str(),
                row.current_context_tokens.map(i64_value).transpose()?,
                row.context_window.map(i64_value).transpose()?,
                row.composition
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|error| RuntimeStoreError::Storage(error.to_string()))?,
                row.source.as_deref(),
                row.updated_at_ms.map(i64_value).transpose()?,
            ],
        )
        .map_err(storage)?;
    Ok(())
}

fn optional_count(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}
