use super::{
    DaemonHostRecord, RuntimeScheduleRecord, RuntimeStoreError, SessionActionRecord,
    SessionActionStatus, SessionIngressCursorStore, SqliteRuntimeStore,
};
use crate::ports::RuntimeStore;
use axon_domain::{DaemonSessionId, GoalId, RuntimeHostId, SessionActionId, SessionRuntimeStatus};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

const HOST_STATE_RUNNING: &str = "running";

pub(crate) fn storage(error: rusqlite::Error) -> RuntimeStoreError {
    RuntimeStoreError::Storage(error.to_string())
}
pub(crate) fn i64_value(value: u64) -> Result<i64, RuntimeStoreError> {
    i64::try_from(value)
        .map_err(|_| RuntimeStoreError::Storage("数值超出 SQLite INTEGER 范围".to_owned()))
}
pub(crate) fn u64_value(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(crate) fn parse_goal_id(value: String) -> rusqlite::Result<GoalId> {
    GoalId::new(value).map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(crate) fn optional_u64(value: Option<i64>) -> rusqlite::Result<Option<u64>> {
    value.map(u64_value).transpose()
}
pub(crate) fn parse_session_id(value: String) -> rusqlite::Result<DaemonSessionId> {
    DaemonSessionId::new(value).map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(crate) fn parse_action_id(value: String) -> rusqlite::Result<SessionActionId> {
    SessionActionId::new(value).map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(crate) fn parse_host_id(value: String) -> rusqlite::Result<RuntimeHostId> {
    RuntimeHostId::new(value).map_err(|_| rusqlite::Error::InvalidQuery)
}
pub(crate) fn parse_runtime_status(value: &str) -> SessionRuntimeStatus {
    match value {
        "idle" => SessionRuntimeStatus::Idle,
        "running" => SessionRuntimeStatus::Running,
        "compacting" => SessionRuntimeStatus::Compacting,
        "waiting" => SessionRuntimeStatus::Waiting,
        "recovering" => SessionRuntimeStatus::Recovering,
        "draining" => SessionRuntimeStatus::Draining,
        "completed" => SessionRuntimeStatus::Completed,
        "failed" => SessionRuntimeStatus::Failed,
        _ => SessionRuntimeStatus::Created,
    }
}

pub(crate) fn parse_action_status(value: &str) -> SessionActionStatus {
    match value {
        "waiting" => SessionActionStatus::Waiting,
        "claimed" => SessionActionStatus::Claimed,
        "running" => SessionActionStatus::Running,
        "committed" => SessionActionStatus::Committed,
        "cancelled" => SessionActionStatus::Cancelled,
        "dead_letter" => SessionActionStatus::DeadLetter,
        _ => SessionActionStatus::Queued,
    }
}

impl SqliteRuntimeStore {
    pub(crate) fn create_session_impl(
        &self,
        session_id: &DaemonSessionId,
        cwd: &str,
        now_ms: u64,
    ) -> Result<crate::domain::runtime::SessionRuntimeRecord, RuntimeStoreError> {
        self.initialize()?;
        let run_session_id = Uuid::new_v4().to_string();
        let inserted = {
            let connection = self.connection()?;
            connection
                .execute("INSERT INTO session_runtimes(session_id,run_session_id,root_session_id,cwd,status,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES(?1,?2,?1,?3,?4,0,0,0,0,?5,?5,?5) ON CONFLICT(session_id) DO NOTHING", params![session_id.as_str(), run_session_id, cwd, SessionRuntimeStatus::Created.as_str(), i64_value(now_ms)?])
                .map_err(storage)?
        };
        if inserted == 0 {
            return self.get_session_impl(session_id);
        }
        Ok(crate::domain::runtime::SessionRuntimeRecord {
            session_id: session_id.clone(),
            run_session_id,
            root_session_id: session_id.clone(),
            cwd: cwd.to_owned(),
            status: SessionRuntimeStatus::Created,
            owner_host_id: None,
            owner_generation: None,
            worker_attempt: 0,
            context_version: 0,
            last_checkpoint_seq: 0,
            last_event_sequence: 0,
            last_activity_at_ms: now_ms,
        })
    }

    pub(crate) fn get_session_impl(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<crate::domain::runtime::SessionRuntimeRecord, RuntimeStoreError> {
        let connection = self.connection()?;
        connection
            .query_row("SELECT session_id,run_session_id,root_session_id,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms FROM session_runtimes WHERE session_id=?1", params![session_id.as_str()], |row| Ok(crate::domain::runtime::SessionRuntimeRecord { session_id: parse_session_id(row.get(0)?)?, run_session_id: row.get(1)?, root_session_id: parse_session_id(row.get(2)?)?, cwd: row.get(3)?, status: parse_runtime_status(&row.get::<_, String>(4)?), owner_host_id: row.get::<_, Option<String>>(5)?.map(parse_host_id).transpose()?, owner_generation: row.get(6)?, worker_attempt: u32::try_from(row.get::<_, i64>(7)?).map_err(|_| rusqlite::Error::InvalidQuery)?, context_version: u64_value(row.get(8)?)?, last_checkpoint_seq: u64_value(row.get(9)?)?, last_event_sequence: u64_value(row.get(10)?)?, last_activity_at_ms: u64_value(row.get(11)?)? })).optional().map_err(storage)?
            .ok_or_else(|| RuntimeStoreError::NotFound(session_id.to_string()))
    }

    pub(crate) fn list_pending_actions_impl(
        &self,
        session_id: &DaemonSessionId,
        limit: u32,
    ) -> Result<Vec<SessionActionRecord>, RuntimeStoreError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT execution_id,goal_id,session_id,kind,priority,CASE status WHEN 'completed' THEN 'committed' WHEN 'failed' THEN 'dead_letter' ELSE status END,dedupe_key,payload_json,goal_revision,context_version,claimed_by,claimed_generation,lease_expires_at_ms,attempt FROM jarvis_executions WHERE session_id=?1 AND status NOT IN ('completed','cancelled','failed') ORDER BY priority DESC,created_at_ms ASC,execution_id ASC LIMIT ?2")
            .map_err(storage)?;
        let rows = statement
            .query_map(
                params![session_id.as_str(), i64::from(limit)],
                Self::read_action,
            )
            .map_err(storage)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage)
    }

    pub(crate) fn ensure_fence(
        transaction: &rusqlite::Transaction<'_>,
        host: &DaemonHostRecord,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        let changed = transaction.execute("UPDATE daemon_hosts SET updated_at_ms = updated_at_ms WHERE host_id = ?1 AND generation = ?2 AND state = ?3 AND lease_expires_at_ms > ?4", params![host.host_id.as_str(), host.generation, HOST_STATE_RUNNING, i64_value(now_ms)?]).map_err(storage)?;
        if changed == 0 {
            return Err(RuntimeStoreError::Fenced);
        }
        Ok(())
    }

    pub(crate) fn read_action(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionActionRecord> {
        Ok(SessionActionRecord {
            action_id: parse_action_id(row.get(0)?)?,
            goal_id: Some(parse_goal_id(row.get(1)?)?),
            session_id: parse_session_id(row.get(2)?)?,
            kind: row.get(3)?,
            priority: row.get(4)?,
            status: parse_action_status(&row.get::<_, String>(5)?),
            dedupe_key: row.get(6)?,
            payload_json: row.get(7)?,
            goal_revision: optional_u64(row.get(8)?)?,
            context_version: optional_u64(row.get(9)?)?,
            claimed_by: row
                .get::<_, Option<String>>(10)?
                .map(parse_host_id)
                .transpose()?,
            claimed_generation: row.get(11)?,
            lease_expires_at_ms: optional_u64(row.get(12)?)?,
            attempt: u32::try_from(row.get::<_, i64>(13)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
        })
    }

    pub(crate) fn read_schedule(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<RuntimeScheduleRecord> {
        Ok(RuntimeScheduleRecord {
            schedule_id: row.get(0)?,
            session_id: row
                .get::<_, Option<String>>(1)?
                .map(parse_session_id)
                .transpose()?,
            kind: row.get(2)?,
            expression: row.get(3)?,
            payload_json: row.get(4)?,
            delivery_mode: row.get(5)?,
            status: row.get(6)?,
            next_run_at_ms: optional_u64(row.get(7)?)?,
            lease_owner: row
                .get::<_, Option<String>>(8)?
                .map(parse_host_id)
                .transpose()?,
            lease_expires_at_ms: optional_u64(row.get(9)?)?,
            failure_count: u32::try_from(row.get::<_, i64>(10)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
        })
    }
}

impl SessionIngressCursorStore for SqliteRuntimeStore {
    fn load_ingress_cursor(
        &self,
        adapter: &str,
        partition: &str,
    ) -> Result<Option<String>, RuntimeStoreError> {
        let connection = self.connection()?;
        connection.query_row("SELECT cursor_json FROM transport_ingress_cursors WHERE adapter=?1 AND partition_key=?2", params![adapter, partition], |row| row.get(0)).optional().map_err(storage)
    }

    fn save_ingress_cursor(
        &self,
        adapter: &str,
        partition: &str,
        cursor_json: &str,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        let connection = self.connection()?;
        connection.execute("INSERT INTO transport_ingress_cursors(adapter,partition_key,cursor_json,updated_at_ms) VALUES(?1,?2,?3,?4) ON CONFLICT(adapter,partition_key) DO UPDATE SET cursor_json=excluded.cursor_json,updated_at_ms=excluded.updated_at_ms", params![adapter, partition, cursor_json, i64_value(now_ms)?]).map_err(storage)?;
        Ok(())
    }
}
