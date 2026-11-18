//! 会话解析：子代理会话与目标执行会话的「先查后建」。
//!
//! 会话标识是 ULID，不携带业务前缀或路径，因此「同一个子代理」只能由
//! `(root_session_id, agent_path)` 认出来，「同一个目标的执行会话」只能由 `goal_id`
//! 认出来。两条路径姿势一致：先按唯一键查，查不到再建一条 ULID，撞唯一键就回读，
//! 因此并发下同一件事不会长出两条会话。

use super::helpers::{SESSION_COLUMNS, normalize_agent_path, session_runtime_row, storage};
use super::{RuntimeStoreError, SqliteRuntimeStore};
use crate::application::new_session_id;
use crate::domain::runtime::{SessionKind, SessionRuntimeRecord, SessionTrustScope};
use crate::ports::{GoalSessionRequest, NewSessionRequest, RuntimeStore, SubagentSessionRequest};
use axon_domain::DaemonSessionId;
use rusqlite::{OptionalExtension, params};

impl SqliteRuntimeStore {
    /// 解析某个根会话下某个代理路径的子代理会话；没有就建一条。
    pub(crate) fn resolve_subagent_session_impl(
        &self,
        request: &SubagentSessionRequest,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
        self.initialize()?;
        let agent_path = normalize_agent_path(&request.agent_path);
        if agent_path.is_empty() {
            return Err(RuntimeStoreError::Storage(
                "子代理会话缺少代理路径".to_owned(),
            ));
        }
        if let Some(found) =
            self.find_subagent_session_impl(&request.root_session_id, &agent_path)?
        {
            return Ok(found);
        }
        let candidate = NewSessionRequest {
            session_id: new_session_id(),
            kind: SessionKind::Subagent,
            trust_scope: SessionTrustScope::Private,
            workspace_id: self.root_workspace_id(&request.root_session_id)?,
            root_session_id: Some(request.root_session_id.clone()),
            agent_path: Some(agent_path.clone()),
            goal_id: None,
            execution_id: None,
            cwd: request.cwd.clone(),
            now_ms: request.now_ms,
        };
        if let Err(error) = self.create_session_impl(&candidate) {
            // 并发下唯一键可能已被另一路占位：回读成功就用它，不把冲突报上去。
            return match self.find_subagent_session_impl(&request.root_session_id, &agent_path)? {
                Some(existing) => Ok(existing),
                None => Err(error),
            };
        }
        self.find_subagent_session_impl(&request.root_session_id, &agent_path)?
            .ok_or_else(|| RuntimeStoreError::Storage("子代理会话登记后回读为空".to_owned()))
    }

    /// 只读查某个根会话下某个代理路径的子代理会话；不存在返回 `None`。
    pub(crate) fn find_subagent_session_impl(
        &self,
        root_session_id: &DaemonSessionId,
        agent_path: &str,
    ) -> Result<Option<SessionRuntimeRecord>, RuntimeStoreError> {
        let agent_path = normalize_agent_path(agent_path);
        if agent_path.is_empty() {
            return Ok(None);
        }
        let connection = self.connection()?;
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM sessions \
             WHERE kind='subagent' AND root_session_id=?1 AND agent_path=?2"
        );
        connection
            .query_row(
                &sql,
                params![root_session_id.as_str(), agent_path],
                session_runtime_row,
            )
            .optional()
            .map_err(storage)
    }

    /// 解析某个目标的执行会话；没有就建一条。
    pub(crate) fn resolve_goal_session_impl(
        &self,
        request: &GoalSessionRequest,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
        self.initialize()?;
        let goal_id = request.goal_id.trim();
        if goal_id.is_empty() {
            return Err(RuntimeStoreError::Storage(
                "目标执行会话缺少目标标识".to_owned(),
            ));
        }
        if let Some(found) = self.find_goal_session_impl(goal_id)? {
            return Ok(found);
        }
        let candidate = NewSessionRequest {
            session_id: new_session_id(),
            kind: SessionKind::Goal,
            trust_scope: SessionTrustScope::Private,
            workspace_id: request.workspace_id.clone(),
            root_session_id: None,
            agent_path: None,
            goal_id: Some(goal_id.to_owned()),
            execution_id: None,
            cwd: request.cwd.clone(),
            now_ms: request.now_ms,
        };
        if let Err(error) = self.create_session_impl(&candidate) {
            return match self.find_goal_session_impl(goal_id)? {
                Some(existing) => Ok(existing),
                None => Err(error),
            };
        }
        self.find_goal_session_impl(goal_id)?
            .ok_or_else(|| RuntimeStoreError::Storage("目标执行会话登记后回读为空".to_owned()))
    }

    fn find_goal_session_impl(
        &self,
        goal_id: &str,
    ) -> Result<Option<SessionRuntimeRecord>, RuntimeStoreError> {
        let connection = self.connection()?;
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM sessions \
             WHERE kind='goal' AND goal_id=?1 ORDER BY created_at_ms ASC, session_id ASC LIMIT 1"
        );
        connection
            .query_row(&sql, params![goal_id], session_runtime_row)
            .optional()
            .map_err(storage)
    }

    /// 子代理会话继承根会话的 Workspace；根会话尚未登记（CLI 直跑）时留空。
    fn root_workspace_id(
        &self,
        root_session_id: &DaemonSessionId,
    ) -> Result<Option<String>, RuntimeStoreError> {
        let connection = self.connection()?;
        let found = connection
            .query_row(
                "SELECT workspace_id FROM sessions WHERE session_id=?1",
                params![root_session_id.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(storage)?;
        Ok(found.flatten())
    }
}
