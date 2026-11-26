//! 投影重建：按会话把三张派生表从事件日志整体重放一遍。
//!
//! 投影是派生数据，日志重排、投影失败与换窗修复之后都可能与日志不一致。这里是唯一的重建
//! 入口：清水位、清派生行，再按日志顺序按批重放。它不在写路径上，触发点是 Daemon 启动对账
//! 与人工故障处理。
//!
//! 重放复用写路径同一份投影实现（[`projection_worker::project_events`]），口径不会分叉。
//!
//! 作业计数不在日志里：它们由宿主按作业事件增量维护，重建前先读出来、重放后写回。

use axon_domain::DaemonSessionId;
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};

use super::super::SqliteRuntimeStore;
use super::projection_watermark;
use super::projection_worker;
use crate::ports::RuntimeStoreError;

/// 单次重放的记录数：与写路径的批次规模同量级，避免一次把整条日志读进内存。
const REBUILD_BATCH_RECORDS: usize = 256;

impl SqliteRuntimeStore {
    /// 重建一个会话的三张投影，返回重放的记录数。
    ///
    /// 先在一个事务里清掉派生行与水位，再按日志重放。清理失败整体回滚；重放中途失败会留下
    /// 部分行，调用方重跑即可：重建每次都从清水位开始，是幂等的。
    ///
    /// 没有事件日志时只清不建：此时正确的派生态就是「没有」，返回 0。
    pub fn rebuild_session_projections(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<u64, RuntimeStoreError> {
        let jobs = self.session_jobs_counts(session_id)?;
        self.clear_session_projections(session_id)?;
        let mut replayed = 0u64;
        let path = self.event_log(session_id).path();
        if path.exists() {
            let mut after = 0u64;
            loop {
                let records = axon_eventlog::read_after(&path, after, REBUILD_BATCH_RECORDS)
                    .map_err(|error| RuntimeStoreError::Storage(error.to_string()))?;
                if records.is_empty() {
                    break;
                }
                let events = records
                    .iter()
                    .map(|record| {
                        (
                            record.event_type.as_str().to_owned(),
                            record.payload.to_string(),
                        )
                    })
                    .collect::<Vec<_>>();
                let origin_channel = records
                    .iter()
                    .find_map(|record| record.origin_channel.clone());
                let now_ms = records.last().map_or(0, |record| record.emitted_at_ms);
                projection_worker::project_events(
                    self,
                    session_id,
                    &events,
                    origin_channel.as_deref(),
                    now_ms,
                )?;
                let last = records.last().map_or(after, |record| record.sequence);
                projection_watermark::advance(self, session_id, last, now_ms);
                replayed += u64::try_from(records.len()).unwrap_or(0);
                if last <= after {
                    break;
                }
                after = last;
            }
        }
        self.restore_session_jobs(session_id, jobs)?;
        Ok(replayed)
    }

    /// Daemon 启动对账：水位超过事件水位的会话，日志必然被截断或整体替换过。
    ///
    /// 启动时是单写者窗口，重建不会与投影 worker 并发；返回重建的会话数。
    pub fn rebuild_rewound_projections(&self) -> Result<usize, RuntimeStoreError> {
        let sessions = self.with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT s.session_id FROM sessions AS s \
                   JOIN projection_state AS p ON p.session_id = s.session_id \
                  WHERE p.last_projected_seq > s.last_event_sequence",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()
        })?;
        let mut rebuilt = 0usize;
        for raw in sessions {
            let Ok(session_id) = DaemonSessionId::new(raw) else {
                continue;
            };
            match self.rebuild_session_projections(&session_id) {
                Ok(_) => rebuilt += 1,
                Err(error) => axon_logger::warning(&format!(
                    "启动投影重建失败，保留原投影: session={} error={error}",
                    session_id.as_str()
                )),
            }
        }
        Ok(rebuilt)
    }

    /// 清掉三张派生表与投影水位；单事务，失败整体回滚。
    fn clear_session_projections(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<(), RuntimeStoreError> {
        self.with_read_connection(|connection| {
            let transaction =
                Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
            for table in [
                "turn_activity",
                "context_snapshots",
                "session_metrics",
                "projection_state",
            ] {
                transaction.execute(
                    &format!("DELETE FROM {table} WHERE session_id=?1"),
                    [session_id.as_str()],
                )?;
            }
            transaction.commit()
        })
    }

    /// 读当前作业计数；没有行时返回 `None`。
    fn session_jobs_counts(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Option<(i64, i64, i64, i64)>, RuntimeStoreError> {
        self.with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT jobs_started, jobs_completed, jobs_failed, jobs_running \
                       FROM session_metrics WHERE session_id=?1",
                    [session_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
        })
    }

    /// 写回重建前读到的作业计数；行不存在时跳过（没有轮次指标的会话本来就没有这一行）。
    fn restore_session_jobs(
        &self,
        session_id: &DaemonSessionId,
        jobs: Option<(i64, i64, i64, i64)>,
    ) -> Result<(), RuntimeStoreError> {
        let Some((started, completed, failed, running)) = jobs else {
            return Ok(());
        };
        self.with_read_connection(|connection| {
            connection.execute(
                "UPDATE session_metrics SET jobs_started=?1, jobs_completed=?2, \
                    jobs_failed=?3, jobs_running=?4 WHERE session_id=?5",
                params![started, completed, failed, running, session_id.as_str()],
            )
        })?;
        Ok(())
    }
}
