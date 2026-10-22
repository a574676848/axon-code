//! Runtime 事实源的健康聚合查询。
//!
//! 全部查询都走既有索引做有界聚合，不读取明细记录，可在值守时安全高频调用。

use super::SqliteRuntimeStore;
use super::helpers::{storage, u64_value};
use crate::ports::{HealthStore, RuntimeHealthCounters, RuntimeStore, RuntimeStoreError};
use axon_domain::{HealthBacklog, HealthStatusCounts};
use rusqlite::Connection;

const INTERACTION_STATUS_SQL: &str =
    "SELECT status,COUNT(*) FROM jarvis_interactions GROUP BY status";
const GOAL_STATUS_SQL: &str = "SELECT status,COUNT(*) FROM jarvis_goals GROUP BY status";
const EXECUTION_STATUS_SQL: &str = "SELECT status,COUNT(*) FROM jarvis_executions GROUP BY status";
const EXECUTION_BACKLOG_SQL: &str = "SELECT COUNT(*),MIN(created_at_ms) FROM jarvis_executions WHERE status IN ('queued','waiting')";

impl HealthStore for SqliteRuntimeStore {
    fn read_runtime_health(&self, now_ms: u64) -> Result<RuntimeHealthCounters, RuntimeStoreError> {
        self.initialize()?;
        let connection = self.connection()?;
        Ok(RuntimeHealthCounters {
            interactions: status_counts(&connection, INTERACTION_STATUS_SQL)?,
            goals: status_counts(&connection, GOAL_STATUS_SQL)?,
            executions: status_counts(&connection, EXECUTION_STATUS_SQL)?,
            execution_backlog: execution_backlog(&connection, now_ms)?,
        })
    }
}

fn status_counts(
    connection: &Connection,
    sql: &str,
) -> Result<HealthStatusCounts, RuntimeStoreError> {
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
    Ok(HealthStatusCounts::from_statuses(statuses))
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
