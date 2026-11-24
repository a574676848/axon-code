//! 会话解析：子代理会话、目标执行会话与定时任务会话的「先查后建」。
//!
//! 会话标识是 ULID，不携带业务前缀或路径，因此「同一个子代理」只能由
//! `(root_session_id, agent_path)` 认出来，「同一个目标的执行会话」只能由 `goal_id`
//! 认出来，「同一个定时任务」只能由 `schedules.session_id` 认出来。三条路径姿势一致：
//! 先按唯一键查，查不到再建一条 ULID，撞唯一键就回读，因此并发下同一件事不会长出两条会话。

use super::helpers::{SESSION_COLUMNS, normalize_agent_path, session_runtime_row, storage};
use super::{RuntimeStoreError, SqliteRuntimeStore};
use crate::application::new_session_id;
use crate::domain::runtime::{SessionKind, SessionRuntimeRecord, SessionTrustScope};
use crate::ports::{
    GoalSessionRequest, NewSessionRequest, RuntimeStore, ScheduleSessionRequest,
    SubagentSessionRequest, WorkspaceStore,
};
use std::collections::HashMap;

use axon_domain::{DaemonSessionId, WorkspaceId};
use rusqlite::{OptionalExtension, params, params_from_iter};

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
        let root_session_id = &request.root_session_id;
        self.resolve_or_create_session(
            |store| store.find_subagent_session_impl(root_session_id, &agent_path),
            || {
                Ok(NewSessionRequest {
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
                })
            },
            "子代理会话登记后回读为空",
        )
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

    /// 按标识批量读取会话行：一次 IN 读回，名册不再逐行回表。
    pub(crate) fn sessions_by_ids_impl(
        &self,
        session_ids: &[DaemonSessionId],
    ) -> Result<HashMap<String, SessionRuntimeRecord>, RuntimeStoreError> {
        if session_ids.is_empty() {
            return Ok(HashMap::new());
        }
        self.initialize()?;
        let connection = self.connection()?;
        let placeholders = (1..=session_ids.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT {} FROM sessions WHERE session_id IN ({placeholders})",
            super::helpers::SESSION_COLUMNS
        );
        let mut statement = connection.prepare(&sql).map_err(storage)?;
        let rows = statement
            .query_map(
                params_from_iter(session_ids.iter().map(DaemonSessionId::as_str)),
                super::helpers::session_runtime_row,
            )
            .map_err(storage)?;
        let mut found = HashMap::with_capacity(session_ids.len());
        for row in rows {
            let record = row.map_err(storage)?;
            found.insert(record.session_id.as_str().to_owned(), record);
        }
        Ok(found)
    }
    /// 按根会话批量读取它最近活动的子代理会话标识。
    ///
    /// 一次查询覆盖一整页目标：查询数与根会话条数无关。同一根有多条子会话时按最近活动
    /// 取一条，因此结果就是「这一轮落在哪条会话」。
    pub(crate) fn latest_subagent_session_ids_impl(
        &self,
        root_session_ids: &[DaemonSessionId],
    ) -> Result<HashMap<String, String>, RuntimeStoreError> {
        if root_session_ids.is_empty() {
            return Ok(HashMap::new());
        }
        self.initialize()?;
        let connection = self.connection()?;
        let placeholders = (1..=root_session_ids.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT root_session_id, session_id FROM sessions \
             WHERE kind='subagent' AND agent_path IS NOT NULL \
               AND root_session_id IN ({placeholders}) \
             ORDER BY last_activity_at_ms DESC, session_id ASC"
        );
        let mut statement = connection.prepare(&sql).map_err(storage)?;
        let rows = statement
            .query_map(
                params_from_iter(root_session_ids.iter().map(DaemonSessionId::as_str)),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(storage)?;
        let mut found: HashMap<String, String> = HashMap::with_capacity(root_session_ids.len());
        for row in rows {
            let (root, child) = row.map_err(storage)?;
            // 排序是最近活动在前，首次出现即该根的最新子会话。
            found.entry(root).or_insert(child);
        }
        Ok(found)
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
        self.resolve_or_create_session(
            |store| store.find_goal_session_impl(goal_id),
            || {
                Ok(NewSessionRequest {
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
                })
            },
            "目标执行会话登记后回读为空",
        )
    }

    /// 解析或登记一条 turn 会话：先按调用方条件查，查不到再建。
    ///
    /// 「查不到才建、并发撞唯一键就回读复用」这段骨架在同构来源上完全一致，区别只在
    /// 查询条件与新建请求。冲突回读必须保留：并发派发时唯一键已被另一路占位，直接把
    /// 冲突报上去会让派发方看到假失败。
    fn resolve_or_create_session(
        &self,
        find: impl Fn(&Self) -> Result<Option<SessionRuntimeRecord>, RuntimeStoreError>,
        candidate: impl FnOnce() -> Result<NewSessionRequest, RuntimeStoreError>,
        empty_reason: &'static str,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
        if let Some(found) = find(self)? {
            return Ok(found);
        }
        let candidate = candidate()?;
        if let Err(error) = self.create_session_impl(&candidate) {
            return match find(self)? {
                Some(existing) => Ok(existing),
                None => Err(error),
            };
        }
        find(self)?.ok_or_else(|| RuntimeStoreError::Storage(empty_reason.to_owned()))
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

    /// 解析定时任务的目标会话：绑定存在就沿用，没有才建一条 `kind=cron` 会话。
    ///
    /// 绑定的事实源是 `schedules.session_id`，这里只读不写：写回由定义侧的
    /// `upsert_schedule` 在同一次落库里完成，因此不存在「任务已建、会话没绑定」的中间态。
    ///
    /// 这里不走上面的骨架：它的「查」读的是任务绑定表而不是会话表，且 cron 会话没有
    /// 唯一键可冲突，回读复用语义不存在。强行合并只会多一次 SELECT 并模糊两条路径。
    pub(crate) fn resolve_schedule_session_impl(
        &self,
        request: &ScheduleSessionRequest,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
        self.initialize()?;
        let schedule_id = request.schedule_id.trim();
        if schedule_id.is_empty() {
            return Err(RuntimeStoreError::Storage(
                "定时任务缺少任务标识".to_owned(),
            ));
        }
        if let Some(session_id) = self.bound_schedule_session(schedule_id)? {
            match self.get_session_impl(&session_id) {
                Ok(record) => return Ok(record),
                // 绑定指向的会话已不存在：按同一条任务重新落一条会话，不报错卡住唤醒。
                Err(RuntimeStoreError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let candidate = NewSessionRequest {
            session_id: new_session_id(),
            kind: SessionKind::Cron,
            trust_scope: SessionTrustScope::Private,
            workspace_id: request.workspace_id.clone(),
            root_session_id: None,
            agent_path: None,
            goal_id: None,
            execution_id: None,
            cwd: self.workspace_cwd(request.workspace_id.as_deref()),
            now_ms: request.now_ms,
        };
        self.create_session_impl(&candidate)
    }

    /// 任务已绑定的会话标识；没有绑定返回 `None`。
    fn bound_schedule_session(
        &self,
        schedule_id: &str,
    ) -> Result<Option<DaemonSessionId>, RuntimeStoreError> {
        let connection = self.connection()?;
        let found = connection
            .query_row(
                "SELECT session_id FROM schedules WHERE schedule_id=?1",
                params![schedule_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(storage)?
            .flatten();
        found
            .map(|value| {
                DaemonSessionId::new(value).map_err(|error| {
                    RuntimeStoreError::Storage(format!("定时任务会话标识非法：{error:?}"))
                })
            })
            .transpose()
    }

    /// 任务归属 Workspace 的根目录；未登记或没有归属时留空。
    ///
    /// 会话的 `cwd` 只是恢复时的提示，不是身份的一部分，因此这里查不到也不报错。
    fn workspace_cwd(&self, workspace_id: Option<&str>) -> String {
        let Some(workspace_id) = workspace_id else {
            return String::new();
        };
        let Ok(workspace_id) = WorkspaceId::new(workspace_id.to_owned()) else {
            return String::new();
        };
        self.get_workspace(&workspace_id)
            .ok()
            .flatten()
            .map_or_else(String::new, |record| record.workspace_root)
    }
}

impl SqliteRuntimeStore {
    /// 推进子代理会话的状态与活跃时间（只作用于 `kind='subagent'` 的行）。
    pub(super) fn mark_subagent_session_status_impl(
        &self,
        session_id: &DaemonSessionId,
        status: &str,
        now_ms: u64,
    ) -> Result<(), crate::ports::RuntimeStoreError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE sessions SET status=?2,last_activity_at_ms=?3,updated_at_ms=?3 \
                 WHERE session_id=?1 AND kind='subagent'",
                params![
                    session_id.as_str(),
                    status,
                    super::helpers::i64_value(now_ms)?
                ],
            )
            .map_err(super::helpers::storage)?;
        Ok(())
    }
    /// 把会话当前轮次标识绑定到这一枚执行标识（只作用于 worker 驱动的会话）。
    pub(super) fn bind_session_execution_impl(
        &self,
        session_id: &DaemonSessionId,
        execution_id: &str,
        now_ms: u64,
    ) -> Result<(), crate::ports::RuntimeStoreError> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE sessions SET execution_id=?2,last_activity_at_ms=?3,updated_at_ms=?3 \
                 WHERE session_id=?1",
                params![
                    session_id.as_str(),
                    execution_id,
                    super::helpers::i64_value(now_ms)?
                ],
            )
            .map_err(super::helpers::storage)?;
        Ok(())
    }
}

impl SqliteRuntimeStore {
    /// 读该会话绑定的目标标识。
    pub(super) fn session_goal_id_impl(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Option<String>, crate::ports::RuntimeStoreError> {
        use rusqlite::OptionalExtension as _;
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT goal_id FROM sessions WHERE session_id=?1",
                params![session_id.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|found| found.flatten())
            .map_err(super::helpers::storage)
    }
}
