//! 会话级上下文快照的读写。
//!
//! 表是投影：一行对应一条会话，只由事件追加的唯一投影入口（`apply_projections`）写入。
//! 写入失败向上返回错误、不阻断落盘：投影水位因此不推进，但后续批次成功会把水位越过
//! 这一批，所以真正可靠的是重建入口而不是「等下一次覆盖」。
//!
//! 读侧对「投影还没追上」有两层兜底：落后时按投影水位回源事件日志尾部，取最近一条用量
//! 读数（见 [`projection_read_fallback`]）；回源也读不到时界面按「未知」降级，绝不用旧值
//! 顶替现在。
//!
//! 读数的口径只有一处：用量帧里的 Provider 实测值。换窗不再写入任何前后读数，
//! 头部因此只展示「最近一次请求占了多少窗口」，剩余量由消费方按窗口与实测占用派生。

use rusqlite::{OptionalExtension, params};

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage};
use crate::domain::ContextSnapshot;
use crate::ports::{ContextSnapshotStore, RuntimeStoreError};
use axon_domain::DaemonSessionId;

/// 投影落后时的读路径回源：快照与会话累计指标共用同一套水位、上界与留痕规则。
///
/// 模块落在同目录的独立文件里，以本文件为声明点：投影读回源是从快照读路径长出来的共用
/// 能力，两条读路径因此共用同一份落后判定。
#[path = "projection_read_fallback.rs"]
mod projection_read_fallback;

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

/// 在调用方的事务里合并读数；与三个投影共用一次提交。
pub(super) fn apply_update_in(
    connection: &rusqlite::Connection,
    session_id: &DaemonSessionId,
    update: &ContextSnapshotUpdate,
) -> Result<(), RuntimeStoreError> {
    if update.is_empty() {
        return Ok(());
    }
    let mut row = read_row(connection, session_id)?.unwrap_or_default();
    if let Some(usage) = update.usage.as_ref() {
        row.current_context_tokens = Some(usage.current_context_tokens);
        row.context_window = usage.context_window;
        // 构成与占用是同一份读数：新的读数没有构成时留空，不拿旧构成拼一个新数字。
        row.composition = usage.composition.clone();
        // 时刻只前进不后退：迟到的旧帧可以更新读数，但不能把「更新于」拉回过去。
        row.updated_at_ms = Some(fresher(row.updated_at_ms, usage.observed_at_ms));
        row.source = Some(SOURCE_USAGE.to_owned());
    }
    write_row(connection, session_id, &row)?;
    Ok(())
}

/// 两条时刻里较新的一条；缺失按「更早」处理。
fn fresher(current: Option<u64>, candidate: u64) -> u64 {
    current.map_or(candidate, |current| current.max(candidate))
}

/// 读取会话级上下文快照；没有记录时返回 `None`。
///
/// 投影是异步的：写返回后立刻读，可能读到上一批甚至空行。因此这里按投影水位判定一次，
/// 落后时按日志尾部定向补一次：只读不写，读不到就退回投影值。
pub(super) fn read_snapshot(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
) -> Result<Option<ContextSnapshot>, RuntimeStoreError> {
    // 连接在作用域里释放：回源要解压事件日志，不该握着池里的连接做文件 IO。
    let (row, lag) = {
        let connection = store.connection()?;
        read_row_and_lag(&connection, session_id)?
    };
    let projected = row.map(snapshot_of);
    let Some(lag) = lag.filter(projection_read_fallback::ProjectionLag::is_behind) else {
        return Ok(projected);
    };
    Ok(projection_read_fallback::freshen_snapshot(
        store, session_id, projected, &lag,
    ))
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

/// 一次查询同时读出快照行与投影水位：落后判定不必再走一次数据库往返。
///
/// 会话行驱动两个按主键的连接：没有快照行时快照列全空，用首列区分「没有行」与「有行但
/// 字段全空」。会话行本身不存在时返回 `(None, None)`。
fn read_row_and_lag(
    connection: &rusqlite::Connection,
    session_id: &DaemonSessionId,
) -> Result<
    (
        Option<SnapshotRow>,
        Option<projection_read_fallback::ProjectionLag>,
    ),
    RuntimeStoreError,
> {
    let found = connection
        .query_row(
            "SELECT (c.session_id IS NOT NULL), \
                    c.current_context_tokens,c.context_window,c.composition_json,c.source,\
                    c.updated_at_ms,s.last_event_sequence, \
                    COALESCE(p.last_projected_seq, 0) \
               FROM sessions AS s \
               LEFT JOIN context_snapshots AS c ON c.session_id = s.session_id \
               LEFT JOIN projection_state AS p ON p.session_id = s.session_id \
              WHERE s.session_id = ?1",
            params![session_id.as_str()],
            |row| {
                let lag = projection_read_fallback::ProjectionLag::new(
                    u64_of(row.get(6)?),
                    u64_of(row.get(7)?),
                );
                let present: i64 = row.get(0)?;
                if present == 0 {
                    return Ok((None, Some(lag)));
                }
                Ok((
                    Some(SnapshotRow {
                        current_context_tokens: optional_count(row.get(1)?),
                        context_window: optional_count(row.get(2)?),
                        composition: row
                            .get::<_, Option<String>>(3)?
                            .and_then(|raw| serde_json::from_str(&raw).ok()),
                        source: row.get(4)?,
                        updated_at_ms: optional_count(row.get(5)?),
                    }),
                    Some(lag),
                ))
            },
        )
        .optional()
        .map_err(storage)?;
    Ok(found.unwrap_or((None, None)))
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

fn u64_of(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}
