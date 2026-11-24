//! Runtime 事实源的健康聚合查询。
//!
//! 全部查询都走既有索引做有界聚合，不读取明细记录，可在值守时安全高频调用。
//!
//! 入站事实的状态由工作项派生，但**不逐行 JOIN**：在途表与历史表各自的索引已经
//! 能给出分组计数，「已收到」用总数相减得到。原先的单条 JOIN + GROUP BY 会把每个
//! 入站行都做两次索引探测与回表，行数一大就退化成正表扫描。

use super::SqliteRuntimeStore;
use super::helpers::{storage, u64_value};
use crate::ports::{HealthStore, RuntimeHealthCounters, RuntimeStore, RuntimeStoreError};
use axon_domain::{HealthBacklog, HealthStatusCounts};
use rusqlite::Connection;

/// 入站事实的「处理中」：只有主 turn 会占用 `running`。
const INTERACTION_PROCESSING_SQL: &str =
    "SELECT COUNT(*) FROM work_items WHERE inbox_id IS NOT NULL AND status='running'";
/// 入站事实的「已收到」：还没有任何工作项（在途或历史）的入站行数。
const INTERACTION_LINKED_SQL: &str = "SELECT (SELECT COUNT(*) FROM work_items WHERE inbox_id IS NOT NULL) \
     + (SELECT COUNT(*) FROM work_item_history WHERE inbox_id IS NOT NULL)";
/// 入站总数：`inbox` 的主键索引即可满足计数。
const INTERACTION_TOTAL_SQL: &str = "SELECT COUNT(*) FROM inbox";
/// 在途工作项里已终态的行，按终态类别分组。
const INTERACTION_TERMINAL_IN_FLIGHT_SQL: &str = "SELECT COALESCE(terminal_kind,status),COUNT(*) \
     FROM work_items WHERE inbox_id IS NOT NULL \
       AND status IN ('completed','failed','cancelled') GROUP BY 1";
/// 历史表里已终态的行，按终态类别分组。
const INTERACTION_TERMINAL_ARCHIVED_SQL: &str =
    "SELECT terminal_kind,COUNT(*) FROM work_item_history GROUP BY 1";
/// 目标按状态分组；`goals_active` 索引可覆盖。
const GOAL_STATUS_SQL: &str = "SELECT status,COUNT(*) FROM goals GROUP BY status";
/// 在途 Execution 按状态分组：表本身只留在途行，规模随活跃度走。
const EXECUTION_IN_FLIGHT_SQL: &str = "SELECT status,COUNT(*) FROM work_items GROUP BY status";
/// 已归档 Execution 按状态分组；`work_item_history_status` 索引可覆盖。
const EXECUTION_ARCHIVED_SQL: &str =
    "SELECT status,COUNT(*) FROM work_item_history GROUP BY status";
/// 积压：排队与等待中的行，走 `work_items_claim` 索引。
const EXECUTION_BACKLOG_SQL: &str =
    "SELECT COUNT(*),MIN(created_at_ms) FROM work_items WHERE status IN ('queued','waiting')";

impl HealthStore for SqliteRuntimeStore {
    fn read_runtime_health(&self, now_ms: u64) -> Result<RuntimeHealthCounters, RuntimeStoreError> {
        self.initialize()?;
        let connection = self.connection()?;
        Ok(RuntimeHealthCounters {
            interactions: interaction_counts(&connection)?,
            goals: HealthStatusCounts::from_statuses(status_pairs(&connection, GOAL_STATUS_SQL)?),
            executions: execution_counts(&connection)?,
            execution_backlog: execution_backlog(&connection, now_ms)?,
        })
    }
}

/// 入站事实的状态分布：处理中、各终态类别、已收到。
fn interaction_counts(connection: &Connection) -> Result<HealthStatusCounts, RuntimeStoreError> {
    let total = count(connection, INTERACTION_TOTAL_SQL)?;
    let processing = count(connection, INTERACTION_PROCESSING_SQL)?;
    let linked = count(connection, INTERACTION_LINKED_SQL)?;
    let mut statuses = Vec::new();
    if processing > 0 {
        statuses.push(("processing".to_owned(), processing));
    }
    statuses.extend(status_pairs(
        connection,
        INTERACTION_TERMINAL_IN_FLIGHT_SQL,
    )?);
    statuses.extend(status_pairs(connection, INTERACTION_TERMINAL_ARCHIVED_SQL)?);
    let received = total.saturating_sub(linked);
    if received > 0 {
        statuses.push(("received".to_owned(), received));
    }
    Ok(HealthStatusCounts::from_statuses(statuses))
}

/// Execution 的状态分布：在途表与历史表分别聚合后相加。
fn execution_counts(connection: &Connection) -> Result<HealthStatusCounts, RuntimeStoreError> {
    let mut statuses = status_pairs(connection, EXECUTION_IN_FLIGHT_SQL)?;
    statuses.extend(status_pairs(connection, EXECUTION_ARCHIVED_SQL)?);
    Ok(HealthStatusCounts::from_statuses(statuses))
}

fn count(connection: &Connection, sql: &str) -> Result<u64, RuntimeStoreError> {
    let value: i64 = connection
        .query_row(sql, [], |row| row.get(0))
        .map_err(storage)?;
    u64_value(value).map_err(storage)
}

fn status_pairs(
    connection: &Connection,
    sql: &str,
) -> Result<Vec<(String, u64)>, RuntimeStoreError> {
    let mut statement = connection.prepare(sql).map_err(storage)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(storage)?;
    let mut statuses = Vec::new();
    for row in rows {
        let (status, count) = row.map_err(storage)?;
        statuses.push((status, u64_value(count).map_err(storage)?));
    }
    Ok(statuses)
}

fn execution_backlog(
    connection: &Connection,
    now_ms: u64,
) -> Result<HealthBacklog, RuntimeStoreError> {
    let (pending, oldest_created_at_ms): (i64, Option<i64>) = connection
        .query_row(EXECUTION_BACKLOG_SQL, [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(storage)?;
    let pending = u64_value(pending).map_err(storage)?;
    if pending == 0 {
        return Ok(HealthBacklog {
            pending: 0,
            oldest_age_ms: None,
        });
    }
    let oldest_age_ms = oldest_created_at_ms
        .map(u64_value)
        .transpose()
        .map_err(storage)?
        .map(|created_at_ms| now_ms.saturating_sub(created_at_ms));
    Ok(HealthBacklog {
        pending,
        oldest_age_ms,
    })
}
