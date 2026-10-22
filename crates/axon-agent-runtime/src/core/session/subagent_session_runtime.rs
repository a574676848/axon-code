//! Subagent 会话生命周期骨架。
//!
//! 该 runtime 把子代理最小运行时语义落到 Axon 现有领域对象上：
//! - 父/子 SessionId 链接
//! - fork 来源标识
//! - 绑定 RuntimeCell 与 LongRunningJob
//! - 完成/取消后的统一状态查询

use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use axon_domain::{
    AxonError, AxonResult, CellId, LongRunningJobId, ModelProfileId, PaneId, PermissionSnapshotId,
    SessionId,
};

use crate::{AgentExecutionId, SubagentToolExecutionReceipt};

/// 子代理 spawn 意图认领结果。
pub enum SubagentSpawnIntentClaim {
    /// 当前调用负责执行真实 spawn。
    Owner,
    /// 等待并复用同一父 turn 内已认领意图的结果。
    Duplicate(SubagentSpawnIntentWaiter),
}

/// 重复 spawn 意图的等待句柄。
pub struct SubagentSpawnIntentWaiter {
    slot: Arc<SubagentSpawnIntentSlot>,
}

impl SubagentSpawnIntentWaiter {
    /// 等待首个 spawn 返回可复用回执。
    #[must_use]
    pub fn wait(self, timeout: Duration) -> Option<Result<SubagentToolExecutionReceipt, String>> {
        let result = self
            .slot
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (result, _) = self
            .slot
            .ready
            .wait_timeout_while(result, timeout, |value| value.is_none())
            .ok()?;
        result.clone()
    }
}

#[derive(Debug)]
struct SubagentSpawnIntentSlot {
    result: Mutex<Option<Result<SubagentToolExecutionReceipt, String>>>,
    ready: Condvar,
}

impl Default for SubagentSpawnIntentSlot {
    fn default() -> Self {
        Self {
            result: Mutex::new(None),
            ready: Condvar::new(),
        }
    }
}

/// 子代理会话已存在。
const SUBAGENT_SESSION_REASON_DUPLICATE_TASK_PATH: &str = "subagent_session:duplicate_task_path";
/// 子代理会话不存在。
const SUBAGENT_SESSION_REASON_NOT_FOUND: &str = "subagent_session:not_found";
/// 子代理会话状态不允许变更。
const SUBAGENT_SESSION_REASON_INVALID_STATE: &str = "subagent_session:invalid_state";

/// 子代理会话状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentSessionStatus {
    /// 子代理会话正在运行。
    Running,
    /// 子代理会话已完成。
    Completed,
    /// 任务未被接受：执行前的准入裁决拒绝了这次子执行。
    ///
    /// 与 [`Self::Errored`] 区分：会话没有真正跑过一轮，父代理不应把它
    /// 读成「执行中出错」。
    Refused,
    /// 子代理会话执行失败。
    Errored,
    /// 子代理会话已取消。
    Cancelled,
}

/// 子代理会话句柄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentSessionHandle {
    /// 父会话 ID。
    pub parent_session_id: SessionId,
    /// 子代理规范路径。
    pub task_path: String,
    /// 子代理会话 ID。
    pub session_id: SessionId,
    /// 当前真实 turn 的执行代际。
    pub execution_id: Option<AgentExecutionId>,
    /// 可选 fork 来源会话 ID。
    pub forked_from_session_id: Option<SessionId>,
    /// 绑定 pane ID。
    pub pane_id: PaneId,
    /// 绑定 RuntimeCell ID。
    pub cell_id: CellId,
    /// 绑定长任务 job ID。
    pub long_running_job_id: LongRunningJobId,
    /// 继承的模型画像 ID。
    pub model_profile_id: ModelProfileId,
    /// 继承的权限快照 ID。
    pub permission_snapshot_id: PermissionSnapshotId,
    /// 当前会话状态。
    pub status: SubagentSessionStatus,
    /// 会话中文摘要。
    pub summary: String,
}

/// 创建子代理会话输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentSessionStartInput {
    /// 父会话 ID。
    pub parent_session_id: SessionId,
    /// 子代理规范路径。
    pub task_path: String,
    /// 可选 fork 来源会话 ID。
    pub forked_from_session_id: Option<SessionId>,
    /// 目标 pane ID。
    pub pane_id: PaneId,
    /// 工作目录。
    pub cwd: String,
    /// 模型画像 ID。
    pub model_profile_id: ModelProfileId,
    /// 权限快照 ID。
    pub permission_snapshot_id: PermissionSnapshotId,
    /// 创建时间戳，毫秒。
    pub created_at_ms: u64,
    /// 初始摘要。
    pub summary: String,
}

/// 子代理会话 runtime。
#[derive(Debug, Default, Clone)]
pub struct SubagentSessionRuntime {
    next_session_id: u64,
    cell_runtime: crate::RuntimeCellRuntime,
    long_running_runtime: crate::LongRunningCellRuntime,
    sessions: Vec<SubagentSessionHandle>,
    spawn_intents: HashMap<String, HashMap<String, Arc<SubagentSpawnIntentSlot>>>,
}

impl SubagentSessionRuntime {
    /// 开始指定父代理的新 turn，并清理该父路径上一 turn 的幂等记录。
    pub fn begin_spawn_intent_turn(&mut self, parent_path: &str) {
        self.spawn_intents
            .insert(parent_path.to_owned(), HashMap::new());
    }

    /// 认领同一父 turn 内的 spawn 意图。
    pub fn claim_spawn_intent(
        &mut self,
        parent_path: &str,
        fingerprint: &str,
    ) -> SubagentSpawnIntentClaim {
        let intents = self
            .spawn_intents
            .entry(parent_path.to_owned())
            .or_default();
        if let Some(slot) = intents.get(fingerprint) {
            return SubagentSpawnIntentClaim::Duplicate(SubagentSpawnIntentWaiter {
                slot: Arc::clone(slot),
            });
        }
        intents.insert(
            fingerprint.to_owned(),
            Arc::new(SubagentSpawnIntentSlot::default()),
        );
        SubagentSpawnIntentClaim::Owner
    }

    /// 发布首个 spawn 的最终结果，唤醒同 turn 内的重复调用。
    pub fn complete_spawn_intent(
        &mut self,
        parent_path: &str,
        fingerprint: &str,
        result: Result<SubagentToolExecutionReceipt, String>,
    ) {
        let Some(slot) = self
            .spawn_intents
            .get(parent_path)
            .and_then(|intents| intents.get(fingerprint))
        else {
            return;
        };
        let mut stored = slot
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *stored = Some(result);
        slot.ready.notify_all();
    }

    /// 启动一个运行中的子代理会话。
    pub fn start(&mut self, input: SubagentSessionStartInput) -> AxonResult<SubagentSessionHandle> {
        if self.session_by_task_path(&input.task_path).is_some() {
            return Err(AxonError::runtime_failure(
                SUBAGENT_SESSION_REASON_DUPLICATE_TASK_PATH,
            ));
        }
        self.next_session_id += 1;
        let session_id = SessionId(self.next_session_id);
        let mut cell = self.cell_runtime.create_cell(
            input.pane_id,
            input.cwd,
            input.model_profile_id,
            input.permission_snapshot_id,
        );
        self.cell_runtime.start_cell(&mut cell)?;
        let long_running_job_id = self.long_running_runtime.attach_running_cell(
            &cell,
            input.created_at_ms,
            input.summary.clone(),
        )?;
        let handle = SubagentSessionHandle {
            parent_session_id: input.parent_session_id,
            task_path: input.task_path,
            session_id,
            execution_id: None,
            forked_from_session_id: input.forked_from_session_id,
            pane_id: input.pane_id,
            cell_id: cell.id,
            long_running_job_id,
            model_profile_id: input.model_profile_id,
            permission_snapshot_id: input.permission_snapshot_id,
            status: SubagentSessionStatus::Running,
            summary: input.summary,
        };
        self.sessions.push(handle.clone());
        Ok(handle)
    }

    /// 为逻辑子代理开始新的真实 turn。
    pub fn begin_execution_by_task_path(
        &mut self,
        task_path: &str,
        execution_id: AgentExecutionId,
        summary: impl Into<String>,
    ) -> AxonResult<SubagentSessionHandle> {
        let handle = self
            .sessions
            .iter_mut()
            .find(|session| session.task_path == task_path)
            .ok_or(AxonError::runtime_failure(
                SUBAGENT_SESSION_REASON_NOT_FOUND,
            ))?;
        handle.status = SubagentSessionStatus::Running;
        handle.execution_id = Some(execution_id);
        handle.summary = summary.into();
        Ok(handle.clone())
    }

    /// 按执行代际收口真实 turn。
    pub fn finish_execution_by_task_path(
        &mut self,
        task_path: &str,
        execution_id: AgentExecutionId,
        status: SubagentSessionStatus,
        summary: impl Into<String>,
    ) -> AxonResult<Option<SubagentSessionHandle>> {
        if status == SubagentSessionStatus::Running {
            return Err(AxonError::runtime_failure(
                SUBAGENT_SESSION_REASON_INVALID_STATE,
            ));
        }
        let Some(handle) = self.sessions.iter_mut().find(|session| {
            session.task_path == task_path
                && session.status == SubagentSessionStatus::Running
                && session.execution_id == Some(execution_id)
        }) else {
            return Ok(None);
        };
        handle.status = status;
        handle.summary = summary.into();
        Ok(Some(handle.clone()))
    }

    /// 标记子代理会话完成。
    pub fn complete(
        &mut self,
        session_id: SessionId,
        summary: impl Into<String>,
    ) -> AxonResult<SubagentSessionHandle> {
        let handle = self.find_mut(session_id)?;
        if handle.status != SubagentSessionStatus::Running {
            return Err(AxonError::runtime_failure(
                SUBAGENT_SESSION_REASON_INVALID_STATE,
            ));
        }
        handle.status = SubagentSessionStatus::Completed;
        handle.summary = summary.into();
        Ok(handle.clone())
    }

    /// 标记子代理会话取消。
    pub fn cancel(
        &mut self,
        session_id: SessionId,
        summary: impl Into<String>,
    ) -> AxonResult<SubagentSessionHandle> {
        let handle = self.find_mut(session_id)?;
        if handle.status != SubagentSessionStatus::Running {
            return Err(AxonError::runtime_failure(
                SUBAGENT_SESSION_REASON_INVALID_STATE,
            ));
        }
        handle.status = SubagentSessionStatus::Cancelled;
        handle.summary = summary.into();
        Ok(handle.clone())
    }

    /// 取消指定路径的运行中子代理会话。
    pub fn cancel_by_task_path(
        &mut self,
        task_path: &str,
        summary: impl Into<String>,
    ) -> AxonResult<Option<SubagentSessionHandle>> {
        let Some(handle) = self.find_running_by_task_path_mut(task_path) else {
            return Ok(None);
        };
        handle.status = SubagentSessionStatus::Cancelled;
        handle.summary = summary.into();
        Ok(Some(handle.clone()))
    }

    /// 完成指定路径的运行中子代理会话。
    pub fn complete_by_task_path(
        &mut self,
        task_path: &str,
        summary: impl Into<String>,
    ) -> AxonResult<Option<SubagentSessionHandle>> {
        let Some(handle) = self.find_running_by_task_path_mut(task_path) else {
            return Ok(None);
        };
        handle.status = SubagentSessionStatus::Completed;
        handle.summary = summary.into();
        Ok(Some(handle.clone()))
    }

    /// 标记指定路径的运行中子代理会话执行失败。
    pub fn fail_by_task_path(
        &mut self,
        task_path: &str,
        summary: impl Into<String>,
    ) -> AxonResult<Option<SubagentSessionHandle>> {
        let Some(handle) = self.find_running_by_task_path_mut(task_path) else {
            return Ok(None);
        };
        handle.status = SubagentSessionStatus::Errored;
        handle.summary = summary.into();
        Ok(Some(handle.clone()))
    }

    /// 取消指定路径子树下的所有运行中子代理会话。
    pub fn cancel_subtree_by_task_path(
        &mut self,
        task_path: &str,
        summary: impl Into<String>,
    ) -> Vec<SubagentSessionHandle> {
        let summary = summary.into();
        let prefix = format!("{task_path}/");
        let mut cancelled = Vec::new();
        for handle in &mut self.sessions {
            if handle.status != SubagentSessionStatus::Running {
                continue;
            }
            if handle.task_path == task_path || handle.task_path.starts_with(&prefix) {
                handle.status = SubagentSessionStatus::Cancelled;
                handle.summary.clone_from(&summary);
                cancelled.push(handle.clone());
            }
        }
        cancelled
    }

    /// 更新指定路径的运行中子代理会话摘要。
    pub fn update_running_summary_by_task_path(
        &mut self,
        task_path: &str,
        summary: impl Into<String>,
    ) -> Option<SubagentSessionHandle> {
        let handle = self.find_running_by_task_path_mut(task_path)?;
        handle.summary = summary.into();
        Some(handle.clone())
    }

    /// 按规范路径查询会话句柄。
    #[must_use]
    pub fn session_by_task_path(&self, task_path: &str) -> Option<&SubagentSessionHandle> {
        self.sessions
            .iter()
            .find(|session| session.task_path == task_path)
    }

    /// 列出所有子代理会话。
    #[must_use]
    pub fn sessions(&self) -> &[SubagentSessionHandle] {
        &self.sessions
    }

    fn find_mut(&mut self, session_id: SessionId) -> AxonResult<&mut SubagentSessionHandle> {
        self.sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
            .ok_or(AxonError::runtime_failure(
                SUBAGENT_SESSION_REASON_NOT_FOUND,
            ))
    }

    fn find_running_by_task_path_mut(
        &mut self,
        task_path: &str,
    ) -> Option<&mut SubagentSessionHandle> {
        self.sessions.iter_mut().find(|session| {
            session.task_path == task_path && session.status == SubagentSessionStatus::Running
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_session_runtime_tracks_parent_and_fork_lineage() {
        let mut runtime = SubagentSessionRuntime::default();
        let handle = runtime
            .start(SubagentSessionStartInput {
                parent_session_id: SessionId(7),
                task_path: "/root/review".to_owned(),
                forked_from_session_id: Some(SessionId(3)),
                pane_id: PaneId(9),
                cwd: "E:\\zbg\\axon-code".to_owned(),
                model_profile_id: ModelProfileId(1),
                permission_snapshot_id: PermissionSnapshotId(2),
                created_at_ms: 1_000,
                summary: "spawned child".to_owned(),
            })
            .unwrap();

        assert_eq!(handle.parent_session_id, SessionId(7));
        assert_eq!(handle.task_path, "/root/review");
        assert_eq!(handle.forked_from_session_id, Some(SessionId(3)));
        assert_eq!(handle.model_profile_id, ModelProfileId(1));
        assert_eq!(handle.permission_snapshot_id, PermissionSnapshotId(2));
        assert_eq!(handle.status, SubagentSessionStatus::Running);
        assert_eq!(handle.execution_id, None);
        assert_eq!(runtime.sessions().len(), 1);
    }

    #[test]
    fn subagent_session_runtime_completes_and_cancels_running_sessions() {
        let mut runtime = SubagentSessionRuntime::default();
        let running = runtime
            .start(SubagentSessionStartInput {
                parent_session_id: SessionId(7),
                task_path: "/root/review".to_owned(),
                forked_from_session_id: None,
                pane_id: PaneId(9),
                cwd: "E:\\zbg\\axon-code".to_owned(),
                model_profile_id: ModelProfileId(1),
                permission_snapshot_id: PermissionSnapshotId(2),
                created_at_ms: 1_000,
                summary: "spawned child".to_owned(),
            })
            .unwrap();
        let completed = runtime
            .complete(running.session_id, "child completed")
            .unwrap();
        assert_eq!(completed.status, SubagentSessionStatus::Completed);
        assert_eq!(completed.summary, "child completed");

        let second = runtime
            .start(SubagentSessionStartInput {
                parent_session_id: SessionId(8),
                task_path: "/root/security".to_owned(),
                forked_from_session_id: None,
                pane_id: PaneId(10),
                cwd: "E:\\zbg\\axon-code".to_owned(),
                model_profile_id: ModelProfileId(1),
                permission_snapshot_id: PermissionSnapshotId(2),
                created_at_ms: 2_000,
                summary: "spawned second child".to_owned(),
            })
            .unwrap();
        let cancelled = runtime.cancel(second.session_id, "user cancelled").unwrap();
        assert_eq!(cancelled.status, SubagentSessionStatus::Cancelled);
    }

    #[test]
    fn subagent_session_runtime_cancels_by_task_path_and_subtree() {
        let mut runtime = SubagentSessionRuntime::default();
        let input = |task_path: &str| SubagentSessionStartInput {
            parent_session_id: SessionId(7),
            task_path: task_path.to_owned(),
            forked_from_session_id: None,
            pane_id: PaneId(9),
            cwd: "E:\\zbg\\axon-code".to_owned(),
            model_profile_id: ModelProfileId(1),
            permission_snapshot_id: PermissionSnapshotId(2),
            created_at_ms: 1_000,
            summary: format!("spawned {task_path}"),
        };

        runtime.start(input("/root/review")).unwrap();
        runtime.start(input("/root/review/security")).unwrap();
        runtime.start(input("/root/build")).unwrap();

        let cancelled = runtime
            .cancel_by_task_path("/root/build", "build interrupted")
            .unwrap()
            .expect("build 会话应被取消");
        assert_eq!(cancelled.status, SubagentSessionStatus::Cancelled);
        assert_eq!(cancelled.summary, "build interrupted");

        let completed = runtime
            .complete_by_task_path("/root/review/security", "security completed")
            .unwrap()
            .expect("security 会话应被完成");
        assert_eq!(completed.status, SubagentSessionStatus::Completed);
        assert_eq!(completed.summary, "security completed");

        let subtree = runtime.cancel_subtree_by_task_path("/root/review", "review closed");
        assert_eq!(subtree.len(), 1);
        assert!(subtree.iter().all(|handle| {
            handle.status == SubagentSessionStatus::Cancelled && handle.summary == "review closed"
        }));
    }

    #[test]
    fn subagent_session_runtime_updates_only_running_summary_by_task_path() {
        let mut runtime = SubagentSessionRuntime::default();
        let input = |task_path: &str| SubagentSessionStartInput {
            parent_session_id: SessionId(7),
            task_path: task_path.to_owned(),
            forked_from_session_id: None,
            pane_id: PaneId(9),
            cwd: "E:\\zbg\\axon-code".to_owned(),
            model_profile_id: ModelProfileId(1),
            permission_snapshot_id: PermissionSnapshotId(2),
            created_at_ms: 1_000,
            summary: format!("spawned {task_path}"),
        };

        runtime.start(input("/root/review")).unwrap();
        let updated = runtime
            .update_running_summary_by_task_path(
                "/root/review",
                "follow-up queued for /root/review",
            )
            .expect("运行中会话应刷新摘要");
        assert_eq!(updated.summary, "follow-up queued for /root/review");

        runtime
            .cancel_by_task_path("/root/review", "review interrupted")
            .unwrap();
        assert!(
            runtime
                .update_running_summary_by_task_path("/root/review", "ignored")
                .is_none()
        );
        assert!(
            runtime
                .update_running_summary_by_task_path("/root/missing", "ignored")
                .is_none()
        );
    }

    #[test]
    fn execution_generation_rejects_stale_completion() {
        let mut runtime = SubagentSessionRuntime::default();
        let handle = runtime
            .start(SubagentSessionStartInput {
                parent_session_id: SessionId(7),
                task_path: "/root/review".to_owned(),
                forked_from_session_id: None,
                pane_id: PaneId(9),
                cwd: "E:\\zbg\\axon-code".to_owned(),
                model_profile_id: ModelProfileId(1),
                permission_snapshot_id: PermissionSnapshotId(2),
                created_at_ms: 1_000,
                summary: "spawned child".to_owned(),
            })
            .unwrap();
        let mut control = crate::AgentControlRuntime::default();
        let review = control
            .spawn(crate::AgentSpawnRequest {
                parent_path: crate::AgentPath::root(),
                task_name: "review".to_owned(),
                message: "审查".to_owned(),
                nickname: None,
                role: None,
            })
            .unwrap();
        let old_lease = control.lease_execution(&review.path).unwrap();
        runtime
            .begin_execution_by_task_path(&handle.task_path, old_lease.execution_id(), "第一轮")
            .unwrap();
        control.interrupt(&review.path).unwrap();
        let new_lease = control.lease_execution(&review.path).unwrap();
        runtime
            .begin_execution_by_task_path(&handle.task_path, new_lease.execution_id(), "第二轮")
            .unwrap();

        let stale = runtime
            .finish_execution_by_task_path(
                &handle.task_path,
                old_lease.execution_id(),
                SubagentSessionStatus::Completed,
                "迟到结果",
            )
            .unwrap();

        assert!(stale.is_none());
        let current = runtime.session_by_task_path(&handle.task_path).unwrap();
        assert_eq!(current.status, SubagentSessionStatus::Running);
        assert_eq!(current.execution_id, Some(new_lease.execution_id()));
        assert_eq!(current.summary, "第二轮");
    }
}
