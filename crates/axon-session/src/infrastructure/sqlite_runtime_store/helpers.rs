use super::{
    DaemonHostRecord, RuntimeScheduleRecord, RuntimeStoreError, SessionActionRecord,
    SessionActionStatus, SessionIngressCursorStore, SessionRuntimeRecord, SqliteRuntimeStore,
};
use crate::application::new_session_id;
use crate::domain::runtime::{SessionKind, SessionTrustScope};
use crate::ports::{NewSessionRequest, RuntimeStore, WorkspaceSessionRequest};
use axon_domain::{DaemonSessionId, GoalId, RuntimeHostId, SessionActionId, SessionRuntimeStatus};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

const HOST_STATE_RUNNING: &str = "running";

/// 会话行的列顺序；`session_runtime_row` 按它取值，任何查询都必须照抄这一份。
pub(crate) const SESSION_COLUMNS: &str = "session_id,current_run_id,root_session_id,agent_path,kind,trust_scope,workspace_id,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms";

/// 按标识读取单个会话。
pub(crate) fn sql_select_session_by_id() -> String {
    format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE session_id=?1")
}

pub(crate) fn parse_session_kind(value: String) -> rusqlite::Result<SessionKind> {
    SessionKind::parse(&value).ok_or(rusqlite::Error::InvalidQuery)
}

/// 代理路径的规范形态：去掉首尾空白与开头的斜杠。
///
/// 各个入口给的是同一个 `AgentPath`，但落地与查询前都先归一，避免 `/root/a`
/// 与 `root/a` 被认成两个子代理。
pub(crate) fn normalize_agent_path(value: &str) -> String {
    value.trim().trim_start_matches('/').to_owned()
}
pub(crate) fn parse_trust_scope(value: String) -> rusqlite::Result<SessionTrustScope> {
    SessionTrustScope::parse(&value).ok_or(rusqlite::Error::InvalidQuery)
}

/// 按 [`SELECT_SESSION`] 的列顺序构造会话记录。
pub(crate) fn session_runtime_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SessionRuntimeRecord> {
    Ok(SessionRuntimeRecord {
        session_id: parse_session_id(row.get(0)?)?,
        current_run_id: row.get(1)?,
        root_session_id: row
            .get::<_, Option<String>>(2)?
            .map(parse_session_id)
            .transpose()?,
        agent_path: row.get(3)?,
        kind: parse_session_kind(row.get(4)?)?,
        trust_scope: parse_trust_scope(row.get(5)?)?,
        workspace_id: row.get(6)?,
        cwd: row.get(7)?,
        status: parse_runtime_status(&row.get::<_, String>(8)?),
        owner_host_id: row
            .get::<_, Option<String>>(9)?
            .map(parse_host_id)
            .transpose()?,
        owner_generation: row.get(10)?,
        worker_attempt: u32::try_from(row.get::<_, i64>(11)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        context_version: u64_value(row.get(12)?)?,
        last_checkpoint_seq: u64_value(row.get(13)?)?,
        last_event_sequence: u64_value(row.get(14)?)?,
        last_activity_at_ms: u64_value(row.get(15)?)?,
    })
}

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
        request: &NewSessionRequest,
    ) -> Result<crate::domain::runtime::SessionRuntimeRecord, RuntimeStoreError> {
        self.initialize()?;
        let current_run_id = Uuid::new_v4().to_string();
        // 代理路径在这一处归一：任何写入口都不会因为开头斜杠不同而落下两份身份。
        let agent_path = request.agent_path.as_deref().map(normalize_agent_path);
        let inserted = {
            let connection = self.connection()?;
            connection
                .execute(
                    "INSERT INTO sessions(session_id,workspace_id,trust_scope,kind,root_session_id,agent_path,goal_id,execution_id,current_run_id,run_generation,title,pinned,archived,sort_order,cwd,status,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,0,NULL,0,0,NULL,?10,?11,0,0,0,0,?12,?12,?12) ON CONFLICT(session_id) DO NOTHING",
                    params![
                        request.session_id.as_str(),
                        request.workspace_id.as_deref(),
                        request.trust_scope.as_str(),
                        request.kind.as_str(),
                        request.root_session_id.as_ref().map(|value| value.as_str()),
                        agent_path.as_deref(),
                        request.goal_id.as_deref(),
                        request.execution_id.as_deref(),
                        current_run_id,
                        request.cwd.as_str(),
                        SessionRuntimeStatus::Created.as_str(),
                        i64_value(request.now_ms)?,
                    ],
                )
                .map_err(storage)?
        };
        if inserted == 0 {
            return self.get_session_impl(&request.session_id);
        }
        Ok(crate::domain::runtime::SessionRuntimeRecord {
            session_id: request.session_id.clone(),
            current_run_id,
            root_session_id: request.root_session_id.clone(),
            agent_path: agent_path.clone(),
            kind: request.kind,
            trust_scope: request.trust_scope,
            workspace_id: request.workspace_id.clone(),
            cwd: request.cwd.clone(),
            status: SessionRuntimeStatus::Created,
            owner_host_id: None,
            owner_generation: None,
            worker_attempt: 0,
            context_version: 0,
            last_checkpoint_seq: 0,
            last_event_sequence: 0,
            last_activity_at_ms: request.now_ms,
        })
    }

    pub(crate) fn resolve_workspace_session_impl(
        &self,
        request: &WorkspaceSessionRequest,
    ) -> Result<crate::domain::runtime::SessionRuntimeRecord, RuntimeStoreError> {
        self.initialize()?;
        let existing = {
            let connection = self.connection()?;
            connection
                .query_row(
                    "SELECT session_id FROM sessions \
                     WHERE workspace_id=?1 AND kind='primary' AND archived=0 \
                     ORDER BY last_activity_at_ms DESC, session_id ASC LIMIT 1",
                    params![request.workspace_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage)?
        };
        if let Some(session_id) = existing {
            let session_id = parse_session_id(session_id).map_err(storage)?;
            return self.get_session_impl(&session_id);
        }
        let mut created =
            NewSessionRequest::primary(new_session_id(), request.cwd.as_str(), request.now_ms);
        created.workspace_id = Some(request.workspace_id.clone());
        created.trust_scope = request.trust_scope;
        self.create_session_impl(&created)
    }

    pub(crate) fn session_id_for_run_impl(
        &self,
        current_run_id: &str,
    ) -> Result<Option<DaemonSessionId>, RuntimeStoreError> {
        let connection = self.connection()?;
        let found = connection
            .query_row(
                "SELECT session_id FROM sessions WHERE current_run_id=?1 LIMIT 1",
                params![current_run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage)?;
        match found {
            Some(session_id) => Ok(Some(parse_session_id(session_id).map_err(storage)?)),
            None => Ok(None),
        }
    }

    pub(crate) fn get_session_impl(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<crate::domain::runtime::SessionRuntimeRecord, RuntimeStoreError> {
        let connection = self.connection()?;
        connection
            .query_row(
                &sql_select_session_by_id(),
                params![session_id.as_str()],
                session_runtime_row,
            )
            .optional()
            .map_err(storage)?
            .ok_or_else(|| RuntimeStoreError::NotFound(session_id.to_string()))
    }

    pub(crate) fn list_pending_actions_impl(
        &self,
        session_id: &DaemonSessionId,
        limit: u32,
    ) -> Result<Vec<SessionActionRecord>, RuntimeStoreError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT execution_id,goal_id,session_id,kind,priority,CASE status WHEN 'completed' THEN 'committed' WHEN 'failed' THEN 'dead_letter' ELSE status END,dedupe_key,payload_json,goal_revision,context_version,claimed_by,claimed_generation,lease_expires_at_ms,attempt FROM work_items WHERE session_id=?1 AND status NOT IN ('completed','cancelled','failed') ORDER BY priority DESC,created_at_ms ASC,execution_id ASC LIMIT ?2")
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
        let changed = transaction.execute("UPDATE daemon_instances SET updated_at_ms = updated_at_ms WHERE host_id = ?1 AND generation = ?2 AND state = ?3 AND lease_expires_at_ms > ?4", params![host.host_id.as_str(), host.generation, HOST_STATE_RUNNING, i64_value(now_ms)?]).map_err(storage)?;
        if changed == 0 {
            return Err(RuntimeStoreError::Fenced);
        }
        Ok(())
    }

    /// 只读 fence 校验：同一判据，但不写 `daemon_instances`。
    ///
    /// 事件写入是最高频的写路径（每个流式帧一批）。每批都 `UPDATE daemon_instances`
    /// 会让租约行随帧数一起进 WAL，把「续租」这条低频写的代价绑到帧率上。这里改为按主键
    /// 读一次租约行：判据完全相同（`state = running` 且未过期的同一代际），fail-closed
    /// 语义不变，但不再产生 WAL 记录。控制面事务（claim / 结算）仍用 `ensure_fence`，
    /// 它们需要与状态更新共用同一次写锁。
    pub(crate) fn check_fence(
        transaction: &rusqlite::Transaction<'_>,
        host: &DaemonHostRecord,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        let active = transaction
            .query_row(
                "SELECT 1 FROM daemon_instances WHERE host_id = ?1 AND generation = ?2 AND state = ?3 AND lease_expires_at_ms > ?4",
                params![
                    host.host_id.as_str(),
                    host.generation,
                    HOST_STATE_RUNNING,
                    i64_value(now_ms)?
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage)?;
        if active.is_none() {
            return Err(RuntimeStoreError::Fenced);
        }
        Ok(())
    }

    pub(crate) fn read_action(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionActionRecord> {
        Ok(SessionActionRecord {
            action_id: parse_action_id(row.get(0)?)?,
            // 会话动作（session_id 类）没有所属目标：goal_id 列为空是常态，
            // 按非空字符串读会让每个会话动作都报「Invalid column type Null」。
            goal_id: row
                .get::<_, Option<String>>(1)?
                .map(parse_goal_id)
                .transpose()?,
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
        connection
            .query_row(
                "SELECT cursor_json FROM ingress_cursors WHERE adapter=?1 AND partition_key=?2",
                params![adapter, partition],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)
    }

    fn save_ingress_cursor(
        &self,
        adapter: &str,
        partition: &str,
        cursor_json: &str,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        let connection = self.connection()?;
        connection.execute("INSERT INTO ingress_cursors(adapter,partition_key,cursor_json,updated_at_ms) VALUES(?1,?2,?3,?4) ON CONFLICT(adapter,partition_key) DO UPDATE SET cursor_json=excluded.cursor_json,updated_at_ms=excluded.updated_at_ms", params![adapter, partition, cursor_json, i64_value(now_ms)?]).map_err(storage)?;
        Ok(())
    }
}
