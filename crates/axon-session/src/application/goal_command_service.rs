//! JARVIS Goal 创建、更新与取消应用服务。

use axon_domain::{
    DaemonSessionId, ExecutionId, ExecutionRecord, GoalCancelCommand, GoalCapabilityError,
    GoalCreateCommand, GoalError, GoalId, GoalPlan, GoalRecord, GoalStatus, GoalUpdateCommand,
    JarvisEvent, JarvisEventId, ScopeRef,
};

use crate::ports::{
    GoalCommitError, GoalCommitStore, GoalMutationStore, GoalStore, GoalStoreError,
    LedgerEventStore, LedgerEventStoreError,
};

/// Goal mutation 服务错误。
#[derive(Debug)]
pub enum GoalCommandServiceError {
    /// 领域规则拒绝。
    Domain(GoalCapabilityError),
    /// Goal 领域构造失败。
    Goal(GoalError),
    /// 存储失败。
    Store(GoalStoreError),
    /// 首次提交事务失败。
    Commit(GoalCommitError),
    /// Event 追加失败。
    Event(LedgerEventStoreError),
}

impl std::fmt::Display for GoalCommandServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(error) => write!(formatter, "Goal 命令被拒绝：{error}"),
            Self::Goal(error) => write!(formatter, "Goal 创建被拒绝：{error}"),
            Self::Store(error) => write!(formatter, "Goal 命令持久化失败：{error}"),
            Self::Commit(error) => write!(formatter, "Goal 首次提交失败：{error}"),
            Self::Event(error) => write!(formatter, "Goal Event 追加失败：{error}"),
        }
    }
}

impl std::error::Error for GoalCommandServiceError {}

/// Goal 首个 Execution 的装配输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalExecutionSpec {
    /// Execution 标识。
    pub execution_id: ExecutionId,
    /// 本次逻辑执行序号，从 1 开始。
    pub attempt: u32,
    /// 内部绑定的运行时会话。
    pub session_id: DaemonSessionId,
    /// 自包含的 Provider turn payload。
    pub payload_json: String,
    /// 同一 Goal 的业务幂等键。
    pub dedupe_key: Option<String>,
}

/// 执行 Goal 更新和取消的应用服务。
#[derive(Debug, Clone)]
pub struct GoalCommandService<S> {
    store: S,
}

impl<S> GoalCommandService<S>
where
    S: GoalStore + GoalMutationStore,
{
    /// 创建命令服务。
    #[must_use]
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// 按 expected revision 更新允许字段。
    pub fn update(
        &self,
        command: &GoalUpdateCommand,
        now_ms: u64,
    ) -> Result<GoalRecord, GoalCommandServiceError> {
        command
            .validate()
            .map_err(GoalCommandServiceError::Domain)?;
        let mut goal = self
            .store
            .get_goal(&command.goal_id)
            .map_err(GoalCommandServiceError::Store)?
            .ok_or_else(|| {
                GoalCommandServiceError::Store(GoalStoreError::NotFound(command.goal_id.clone()))
            })?;
        if goal.revision != command.expected_revision {
            return Err(GoalCommandServiceError::Domain(
                GoalCapabilityError::RevisionConflict {
                    expected: command.expected_revision,
                    actual: goal.revision,
                },
            ));
        }
        if goal.status.is_terminal() {
            return Err(GoalCommandServiceError::Domain(
                GoalCapabilityError::TerminalGoal,
            ));
        }
        if let Some(title) = &command.title {
            if title.trim().is_empty() {
                return Err(GoalCommandServiceError::Domain(
                    GoalCapabilityError::EmptyUpdate,
                ));
            }
            goal.title = title.clone();
        }
        if let Some(waiting_reason) = &command.waiting_reason {
            goal.waiting_reason = Some(waiting_reason.clone());
        }
        if let Some(next_step) = &command.next_step {
            goal.next_step = Some(next_step.clone());
        }
        if let Some(priority) = command.priority {
            goal.priority = priority;
        }
        if let Some(status) = command.status {
            if !matches!(status, GoalStatus::Waiting | GoalStatus::Paused) {
                return Err(GoalCommandServiceError::Store(GoalStoreError::Conflict(
                    "状态更新只允许等待或暂停；启动、取消和验收请使用对应操作".to_owned(),
                )));
            }
            goal.transition(status, now_ms)
                .map_err(|_| GoalCommandServiceError::Domain(GoalCapabilityError::TerminalGoal))?;
        } else {
            goal.revision = goal.revision.saturating_add(1);
            goal.updated_at_ms = now_ms;
        }
        self.store
            .update_goal_with_event(
                &goal,
                command.expected_revision,
                &goal_updated_event(&goal, now_ms).map_err(|error| {
                    GoalCommandServiceError::Store(GoalStoreError::Conflict(error))
                })?,
            )
            .map_err(GoalCommandServiceError::Store)?;
        Ok(goal)
    }

    /// 原子语义取消 Goal；终态取消不产生副作用。
    pub fn cancel(
        &self,
        command: &GoalCancelCommand,
        now_ms: u64,
    ) -> Result<GoalRecord, GoalCommandServiceError> {
        command
            .validate()
            .map_err(GoalCommandServiceError::Domain)?;
        let mut goal = self
            .store
            .get_goal(&command.goal_id)
            .map_err(GoalCommandServiceError::Store)?
            .ok_or_else(|| {
                GoalCommandServiceError::Store(GoalStoreError::NotFound(command.goal_id.clone()))
            })?;
        if goal.revision != command.expected_revision {
            return Err(GoalCommandServiceError::Domain(
                GoalCapabilityError::RevisionConflict {
                    expected: command.expected_revision,
                    actual: goal.revision,
                },
            ));
        }
        if goal.status.is_terminal() {
            if goal.status == GoalStatus::Cancelled {
                return Ok(goal);
            }
            return Err(GoalCommandServiceError::Domain(
                GoalCapabilityError::CancellationConflict,
            ));
        }
        goal.transition(GoalStatus::Cancelled, now_ms)
            .map_err(|_| {
                GoalCommandServiceError::Domain(GoalCapabilityError::CancellationConflict)
            })?;
        goal.cancellation_reason = Some(command.reason.clone());
        goal.cancellation_requested_at_ms = Some(now_ms);
        self.store
            .cancel_goal(&goal, command.expected_revision, &command.reason, now_ms)
            .map_err(GoalCommandServiceError::Store)?;
        Ok(goal)
    }
}

impl<S> GoalCommandService<S>
where
    S: GoalStore + GoalCommitStore,
{
    /// 原子启动已有 Goal，并创建一个绑定同一 Goal 的 queued Execution。
    pub fn start(
        &self,
        goal_id: &GoalId,
        expected_revision: u64,
        execution: &GoalExecutionSpec,
        event: &JarvisEvent,
        now_ms: u64,
    ) -> Result<GoalRecord, GoalCommandServiceError> {
        let Some(mut goal) = self
            .store
            .get_goal(goal_id)
            .map_err(GoalCommandServiceError::Store)?
        else {
            return Err(GoalCommandServiceError::Store(GoalStoreError::NotFound(
                goal_id.clone(),
            )));
        };
        if goal.revision != expected_revision {
            return Err(GoalCommandServiceError::Domain(
                GoalCapabilityError::RevisionConflict {
                    expected: expected_revision,
                    actual: goal.revision,
                },
            ));
        }
        if !matches!(
            goal.status,
            axon_domain::GoalStatus::Proposed
                | axon_domain::GoalStatus::Waiting
                | axon_domain::GoalStatus::Paused
        ) {
            return Err(GoalCommandServiceError::Domain(
                GoalCapabilityError::CancellationConflict,
            ));
        }
        goal.transition(axon_domain::GoalStatus::Active, now_ms)
            .map_err(GoalCommandServiceError::Goal)?;
        let mut execution_record =
            ExecutionRecord::new(execution.execution_id.clone(), goal.goal_id.clone(), now_ms)
                .with_session_id(execution.session_id.clone())
                .with_payload_json(execution.payload_json.clone());
        execution_record.attempt = execution.attempt;
        execution_record.dedupe_key = execution.dedupe_key.clone();
        self.store
            .commit_existing_goal_start(&goal, expected_revision, &execution_record, event)
            .map_err(GoalCommandServiceError::Commit)?;
        Ok(goal)
    }
}

impl<S> GoalCommandService<S>
where
    S: GoalStore + GoalCommitStore + LedgerEventStore,
{
    /// 创建 Goal，并按需原子提交首个 queued Execution 与 GoalCreated Event。
    ///
    /// 携带 Execution 时通过 `commit_goal_start` 单事务提交；不携带 Execution
    /// 时先幂等落 Goal，再追加幂等 Event。
    pub fn create(
        &self,
        command: &GoalCreateCommand,
        goal_id: GoalId,
        scope: &ScopeRef,
        execution: Option<&GoalExecutionSpec>,
        event: &JarvisEvent,
        now_ms: u64,
    ) -> Result<GoalRecord, GoalCommandServiceError> {
        command
            .validate()
            .map_err(GoalCommandServiceError::Domain)?;
        let goal = GoalRecord::new(
            goal_id,
            scope.clone(),
            command.title.clone(),
            GoalPlan {
                objective: command.objective.clone(),
                steps: command.steps.clone(),
            },
            command.acceptance.clone(),
            now_ms,
        )
        .map_err(GoalCommandServiceError::Goal)?;
        match execution {
            Some(spec) => {
                let mut execution =
                    ExecutionRecord::new(spec.execution_id.clone(), goal.goal_id.clone(), now_ms)
                        .with_session_id(spec.session_id.clone())
                        .with_payload_json(spec.payload_json.clone());
                execution.dedupe_key = spec.dedupe_key.clone();
                self.store
                    .commit_goal_start(&goal, &execution, event)
                    .map_err(GoalCommandServiceError::Commit)?;
            }
            None => {
                self.store
                    .commit_goal_proposal(&goal, event)
                    .map_err(GoalCommandServiceError::Commit)?;
            }
        }
        Ok(goal)
    }
}

fn goal_updated_event(goal: &GoalRecord, now_ms: u64) -> Result<JarvisEvent, String> {
    let goal_id = goal.goal_id.as_str();
    let revision = goal.revision;
    Ok(JarvisEvent {
        event_id: JarvisEventId::new(format!("goal-update-{goal_id}-{revision}"))
            .map_err(|error| error.to_string())?,
        sequence: 0,
        event_type: "goal.updated".to_owned(),
        idempotency_key: format!("goal.updated:{goal_id}:{revision}"),
        cause_event_id: None,
        payload_json: serde_json::json!({
            "goal_id": goal_id,
            "revision": revision,
            "status": goal.status,
        })
        .to_string(),
        occurred_at_ms: now_ms,
    })
}
