//! 主 turn Goal 工具的 Daemon 命令映射。
//!
//! Daemon 是 Goal mutation 的唯一写入口。工具执行进程只提供已解析的领域命令
//! 与运行时绑定的 Workspace；本层负责重新读取权威事实、校验可见作用域、
//! 调用应用服务，并返回结构化回执供主 turn 自然语言解释。

use super::goal_execution;
use super::{DaemonHost, DaemonHostError};
use crate::ports::ProcessIdentity;
use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DecisionEffect, DecisionId, DecisionRecord,
    DisclosurePolicy, ExecutionId, GoalCancelCommand, GoalCreateCommand, GoalCreationScope,
    GoalDetailQuery, GoalId, GoalUpdateCommand, JarvisEvent, JarvisEventId, ScopeRef,
};
use axon_protocol::{GoalSource, GoalStartRequest, GoalToolCommand, GoalToolRequest};
use axon_session::WorkspaceRecord;
use axon_session::{GoalCommandService, GoalExecutionSpec, GoalQueryService, RuntimeStore};
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};
use serde_json::{Value, json};

/// Goal 创建事件的稳定类型。
const GOAL_CREATED_EVENT_TYPE: &str = "goal.created";
/// 工具驱动 Goal 的派生标识前缀。
const GOAL_ID_PREFIX: &str = "goal-";
const GOAL_EXECUTION_ID_PREFIX: &str = "execution-";
const GOAL_SESSION_ID_PREFIX: &str = "goal-session-";
const GOAL_EVENT_ID_PREFIX: &str = "event-";
const GOAL_DECISION_ID_PREFIX: &str = "decision-";
/// scope 内尚无 Decision 时的期望 revision 基线。
const EMPTY_DECISION_REVISION: u64 = 0;
/// 不可见与不存在使用同一回执，避免受限受众探测私密 Goal。
const GOAL_NOT_VISIBLE: &str = "Goal 不存在或对当前作用域不可见";
/// 群聊等 Workspace 受众不能创建或查询 Global Goal。
const GOAL_GLOBAL_NOT_ALLOWED: &str = "当前受众只能访问 Workspace Goal";

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 执行一次主 turn Goal 工具命令。
    pub(super) fn goal_tool_value(
        &self,
        request: GoalToolRequest,
    ) -> Result<Value, DaemonHostError> {
        let actor = request.actor.trim().to_owned();
        let idempotency_key = request.idempotency_key.trim().to_owned();
        if actor.is_empty() || idempotency_key.is_empty() {
            return Err(DaemonHostError::Worker(
                "Goal 工具请求缺少 actor 或幂等键".to_owned(),
            ));
        }
        let now_ms = self.clock.now_ms();
        let workspace = self.bound_workspace(request.workspace_id.as_deref())?;
        let source = request.source.as_ref();
        let disclosure_policy = request.disclosure_policy;
        let transport_context = request.transport_context.as_ref();
        match request.command {
            GoalToolCommand::Create(command) => self.goal_create_value(
                &actor,
                &idempotency_key,
                source,
                &command,
                workspace.as_ref(),
                disclosure_policy,
                transport_context,
                now_ms,
            ),
            GoalToolCommand::List(query) => {
                self.goal_list_value(&query, workspace.as_ref(), disclosure_policy)
            }
            GoalToolCommand::Get(query) => {
                self.goal_get_value(&query, workspace.as_ref(), disclosure_policy)
            }
            GoalToolCommand::Update(command) => {
                self.goal_update_value(&command, workspace.as_ref(), disclosure_policy, now_ms)
            }
            GoalToolCommand::Cancel(command) => {
                self.goal_cancel_value(&command, workspace.as_ref(), disclosure_policy, now_ms)
            }
            GoalToolCommand::Start(command) => self.start_goal_value(GoalStartRequest {
                goal_id: command.goal_id,
                expected_revision: command.expected_revision,
                actor,
                workspace_id: request.workspace_id,
                idempotency_key,
                source: request.source,
                disclosure_policy,
                transport_context: request.transport_context,
            }),
        }
    }

    /// 解析运行时绑定的 Workspace；未注册时降级为无 Workspace 上下文。
    ///
    /// 从用户主目录启动且尚未注册 Workspace 时，全局 Goal 必须保持可用；
    /// 只有显式要求 `current_workspace` 的查询或创建才会拒绝。
    fn bound_workspace(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<Option<WorkspaceRecord>, DaemonHostError> {
        let Some(workspace_id) = workspace_id else {
            return Ok(None);
        };
        let workspace_id = axon_domain::WorkspaceId::new(workspace_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        self.store
            .get_workspace(&workspace_id)
            .map_err(goal_service_error)
    }

    /// 列表查询：解析显式 scope 后读取可见 Goal 摘要。
    fn goal_list_value(
        &self,
        query: &axon_domain::GoalListQuery,
        workspace: Option<&WorkspaceRecord>,
        disclosure_policy: Option<DisclosurePolicy>,
    ) -> Result<Value, DaemonHostError> {
        let scopes = visible_scope_set(&query.scope, workspace, disclosure_policy)?;
        let page = GoalQueryService::new(self.store.clone())
            .list(&scopes, query)
            .map_err(goal_service_error)?;
        Ok(json!({
            "tool": "goal_list",
            "goals": page.goals,
            "nextCursor": page.next_cursor,
        }))
    }

    fn goal_get_value(
        &self,
        query: &GoalDetailQuery,
        workspace: Option<&WorkspaceRecord>,
        disclosure_policy: Option<DisclosurePolicy>,
    ) -> Result<Value, DaemonHostError> {
        let scopes = visible_scopes(workspace, disclosure_policy)?;
        let view = GoalQueryService::new(self.store.clone())
            .get(&scopes, query)
            .map_err(goal_service_error)?
            .ok_or_else(|| DaemonHostError::Worker(GOAL_NOT_VISIBLE.to_owned()))?;
        Ok(json!({"tool": "goal_get", "activity": view}))
    }

    fn goal_create_value(
        &self,
        actor: &str,
        idempotency_key: &str,
        source: Option<&GoalSource>,
        command: &GoalCreateCommand,
        workspace: Option<&WorkspaceRecord>,
        disclosure_policy: Option<DisclosurePolicy>,
        transport_context: Option<&Value>,
        now_ms: u64,
    ) -> Result<Value, DaemonHostError> {
        command.validate().map_err(goal_service_error)?;
        let scope = match command.scope {
            GoalCreationScope::Global => {
                if disclosure_policy.is_some_and(|policy| policy != DisclosurePolicy::PrivateOnly) {
                    return Err(DaemonHostError::Worker(GOAL_GLOBAL_NOT_ALLOWED.to_owned()));
                }
                ScopeRef::global()
            }
            GoalCreationScope::CurrentWorkspace => {
                let workspace = workspace.ok_or_else(|| {
                    DaemonHostError::Worker("创建 Workspace Goal 需要绑定 Workspace".to_owned())
                })?;
                ScopeRef::workspace(workspace.workspace_id.clone())
                    .map_err(|error| DaemonHostError::Worker(error.to_string()))?
            }
        };
        let goal_id = GoalId::new(format!("{GOAL_ID_PREFIX}{idempotency_key}"))
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        if self
            .store
            .get_goal(&goal_id)
            .map_err(goal_service_error)?
            .is_some()
        {
            let existing = self.ensure_goal_visible(&goal_id, workspace, disclosure_policy)?;
            let execution = self
                .store
                .execution_for_goal_dedupe(&goal_id, idempotency_key)
                .map_err(goal_service_error)?;
            if existing.scope != scope
                || existing.title != command.title
                || existing.plan.objective != command.objective
                || existing.plan.steps != command.steps
                || existing.acceptance != command.acceptance
                || execution.is_some() != command.create_execution
            {
                return Err(DaemonHostError::Worker(
                    "幂等键已绑定不同的任务内容或作用域".to_owned(),
                ));
            }
            let mut receipt = goal_receipt("goal_create", &existing, false);
            receipt["executionId"] = json!(execution.as_ref().map(|item| &item.execution_id));
            receipt["started"] = json!(execution.is_some());
            return Ok(receipt);
        }
        let execution = self.new_goal_execution(
            actor,
            source,
            command,
            workspace,
            disclosure_policy,
            transport_context,
            &goal_id,
            now_ms,
            Some(idempotency_key),
            None,
        )?;
        if execution.is_some() {
            // Worker 门禁要求 Goal 作用域存在有效 Decision；创建即授权，
            // 与主 turn 创建路径一致，避免首个 queued Execution 被门禁取消。
            self.ensure_allow_decision(
                &scope,
                actor,
                Self::goal_decision_id(&goal_id)?,
                None,
                now_ms,
            )?;
        }
        let event = goal_created_event(&scope, command, &goal_id, now_ms)?;
        let goal = GoalCommandService::new(self.store.clone())
            .create(command, goal_id, &scope, execution.as_ref(), &event, now_ms)
            .map_err(goal_service_error)?;
        let mut receipt = goal_receipt("goal_create", &goal, true);
        receipt["executionId"] = json!(execution.as_ref().map(|item| &item.execution_id));
        receipt["started"] = json!(execution.is_some());
        Ok(receipt)
    }

    /// 受控启动已有 Goal：创建 queued Execution 与 active 状态必须同一事务提交。
    pub(super) fn start_goal_value(
        &self,
        request: GoalStartRequest,
    ) -> Result<Value, DaemonHostError> {
        let actor = request.actor.trim().to_owned();
        let idempotency_key = request.idempotency_key.trim().to_owned();
        if actor.is_empty() || idempotency_key.is_empty() {
            return Err(DaemonHostError::Worker(
                "Goal 启动请求缺少 actor 或幂等键".to_owned(),
            ));
        }
        let goal_id = GoalId::new(request.goal_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let workspace = self.bound_workspace(request.workspace_id.as_deref())?;
        let goal =
            self.ensure_goal_visible(&goal_id, workspace.as_ref(), request.disclosure_policy)?;
        if let Some(existing) = self
            .store
            .execution_for_goal_dedupe(&goal_id, &idempotency_key)
            .map_err(goal_service_error)?
        {
            return Ok(json!({
                "tool": "goal_start",
                "goalId": goal.goal_id,
                "status": goal.status,
                "revision": goal.revision,
                "executionId": existing.execution_id,
                "started": false,
                "idempotent": true,
            }));
        }
        if goal.revision != request.expected_revision
            || !matches!(
                goal.status,
                axon_domain::GoalStatus::Proposed
                    | axon_domain::GoalStatus::Waiting
                    | axon_domain::GoalStatus::Paused
            )
        {
            return Err(DaemonHostError::Worker(
                "Goal 版本或状态已变化，请读取最新进度".to_owned(),
            ));
        }
        let prior = self
            .store
            .list_executions_for_goal(&goal_id, 1)
            .map_err(goal_service_error)?;
        let inherited = prior
            .first()
            .map(|item| serde_json::from_str::<Value>(&item.payload_json))
            .transpose()
            .map_err(goal_service_error)?
            .and_then(|payload| payload.get("transportContext").cloned());
        let command = GoalCreateCommand {
            scope: match &goal.scope {
                ScopeRef::Global => GoalCreationScope::Global,
                ScopeRef::Workspace(_) => GoalCreationScope::CurrentWorkspace,
            },
            title: goal.title.clone(),
            objective: goal.plan.objective.clone(),
            steps: goal.plan.steps.clone(),
            acceptance: goal.acceptance.clone(),
            create_execution: true,
        };
        let execution = self
            .new_goal_execution(
                &actor,
                request.source.as_ref(),
                &command,
                workspace.as_ref(),
                request.disclosure_policy,
                inherited.as_ref().or(request.transport_context.as_ref()),
                &goal_id,
                self.clock.now_ms(),
                Some(idempotency_key.as_str()),
                Some(goal.revision.saturating_add(1)),
            )?
            .ok_or_else(|| DaemonHostError::Worker("Goal 启动未生成 Execution".to_owned()))?;
        self.ensure_allow_decision(
            &goal.scope,
            &actor,
            Self::goal_decision_id(&goal_id)?,
            None,
            self.clock.now_ms(),
        )?;
        let now_ms = self.clock.now_ms();
        let event = goal_started_event(&goal_id, goal.revision.saturating_add(1), now_ms)?;
        let started = GoalCommandService::new(self.store.clone())
            .start(
                &goal_id,
                request.expected_revision,
                &execution,
                &event,
                now_ms,
            )
            .map_err(goal_service_error)?;
        Ok(json!({
            "tool": "goal_start",
            "goalId": started.goal_id,
            "status": started.status,
            "revision": started.revision,
            "executionId": execution.execution_id,
            "started": true,
        }))
    }

    /// Goal 绑定 Decision 标识；同一 Goal 复用同一决策，避免重复授权。
    fn goal_decision_id(goal_id: &GoalId) -> Result<DecisionId, DaemonHostError> {
        DecisionId::new(format!("{GOAL_DECISION_ID_PREFIX}{goal_id}"))
            .map_err(|error| DaemonHostError::Worker(error.to_string()))
    }

    /// 在作用域内追加 Allow Decision；已存在同 ID 决策时复用并返回 `false`。
    fn ensure_allow_decision(
        &self,
        scope: &ScopeRef,
        actor: &str,
        decision_id: DecisionId,
        expires_at_ms: Option<u64>,
        now_ms: u64,
    ) -> Result<(DecisionId, bool), DaemonHostError> {
        let latest = self
            .store
            .get_latest_decision(scope, now_ms)
            .map_err(goal_service_error)?;
        if let Some(existing) = latest
            .as_ref()
            .filter(|decision| decision.decision_id == decision_id)
        {
            return Ok((existing.decision_id.clone(), false));
        }
        let expected_revision =
            latest.map_or(EMPTY_DECISION_REVISION, |decision| decision.revision);
        let revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| DaemonHostError::Worker("Decision revision 溢出".to_owned()))?;
        let decision = DecisionRecord {
            decision_id: decision_id.clone(),
            scope: scope.clone(),
            actor: actor.to_owned(),
            effect: DecisionEffect::Allow,
            revision,
            expires_at_ms,
            created_at_ms: now_ms,
        };
        self.store
            .append_decision(&decision, expected_revision)
            .map_err(goal_service_error)?;
        Ok((decision_id, true))
    }

    /// 装配首个 queued Execution；未请求执行时返回 None。
    fn new_goal_execution(
        &self,
        _actor: &str,
        source: Option<&GoalSource>,
        command: &GoalCreateCommand,
        workspace: Option<&WorkspaceRecord>,
        disclosure_policy: Option<DisclosurePolicy>,
        transport_context: Option<&Value>,
        goal_id: &GoalId,
        now_ms: u64,
        dedupe_key: Option<&str>,
        execution_revision: Option<u64>,
    ) -> Result<Option<GoalExecutionSpec>, DaemonHostError> {
        if !command.create_execution {
            return Ok(None);
        }
        let workspace = workspace.ok_or_else(|| {
            DaemonHostError::Worker("后台 Execution 必须绑定 Workspace".to_owned())
        })?;
        let session_id = DaemonSessionId::new(format!("{GOAL_SESSION_ID_PREFIX}{goal_id}"))
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let prompt = goal_execution::prompt(
            &command.title,
            &command.objective,
            &command.steps,
            &command.acceptance,
        );
        let source = source.cloned().unwrap_or(GoalSource::OneOff);
        let mut transport_context = match &source {
            GoalSource::AxonCron {
                channel,
                destination,
                ..
            } => json!({
                "channel": channel,
                "destination": destination,
                "source": source,
                "audience": AudienceContext::new(
                    AudienceKind::Private,
                    disclosure_policy.unwrap_or(DisclosurePolicy::PrivateOnly),
                ),
            }),
            GoalSource::OneOff => transport_context.cloned().ok_or_else(|| {
                DaemonHostError::Worker("启动 Goal 需要可恢复的回复路由".to_owned())
            })?,
        };
        let audience: AudienceContext = serde_json::from_value(
            transport_context
                .get("audience")
                .cloned()
                .ok_or_else(|| DaemonHostError::Worker("Goal 缺少受众".to_owned()))?,
        )
        .map_err(goal_service_error)?;
        if audience.disclosure() != disclosure_policy.unwrap_or(DisclosurePolicy::PrivateOnly) {
            return Err(DaemonHostError::Worker(
                "Goal 受众披露边界与请求不一致".to_owned(),
            ));
        }
        for field in ["channel", "destination"] {
            if transport_context
                .get(field)
                .and_then(Value::as_str)
                .is_none_or(|item| item.trim().is_empty())
            {
                return Err(DaemonHostError::Worker(format!(
                    "Goal 回复路由缺少 {field}"
                )));
            }
        }
        transport_context["workspaceId"] = json!(workspace.workspace_id);
        transport_context["goalId"] = json!(goal_id);
        transport_context["stage"] = json!("initial");
        transport_context["source"] = json!(source);
        self.store
            .create_session(&session_id, &workspace.workspace_root, now_ms)
            .map_err(goal_service_error)?;
        Ok(Some(GoalExecutionSpec {
            execution_id: ExecutionId::new(execution_revision.map_or_else(
                || format!("{GOAL_EXECUTION_ID_PREFIX}{goal_id}"),
                |revision| format!("{GOAL_EXECUTION_ID_PREFIX}{goal_id}-r{revision}"),
            ))
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?,
            attempt: 1,
            session_id,
            payload_json: goal_execution::payload(&prompt, transport_context),
            dedupe_key: dedupe_key.map(str::to_owned),
        }))
    }

    fn goal_update_value(
        &self,
        command: &GoalUpdateCommand,
        workspace: Option<&WorkspaceRecord>,
        disclosure_policy: Option<DisclosurePolicy>,
        now_ms: u64,
    ) -> Result<Value, DaemonHostError> {
        self.ensure_goal_visible(&command.goal_id, workspace, disclosure_policy)?;
        let goal = GoalCommandService::new(self.store.clone())
            .update(command, now_ms)
            .map_err(goal_service_error)?;
        Ok(goal_receipt("goal_update", &goal, true))
    }

    fn goal_cancel_value(
        &self,
        command: &GoalCancelCommand,
        workspace: Option<&WorkspaceRecord>,
        disclosure_policy: Option<DisclosurePolicy>,
        now_ms: u64,
    ) -> Result<Value, DaemonHostError> {
        self.ensure_goal_visible(&command.goal_id, workspace, disclosure_policy)?;
        let goal = GoalCommandService::new(self.store.clone())
            .cancel(command, now_ms)
            .map_err(goal_service_error)?;
        Ok(json!({
            "tool": "goal_cancel",
            "goalId": goal.goal_id,
            "status": goal.status,
            "revision": goal.revision,
            "cancellationReason": goal.cancellation_reason,
        }))
    }

    /// 重新读取权威 Goal 并校验可见作用域。
    fn ensure_goal_visible(
        &self,
        goal_id: &GoalId,
        workspace: Option<&WorkspaceRecord>,
        disclosure_policy: Option<DisclosurePolicy>,
    ) -> Result<axon_domain::GoalRecord, DaemonHostError> {
        let scopes = visible_scopes(workspace, disclosure_policy)?;
        self.store
            .get_goal(goal_id)
            .map_err(goal_service_error)?
            .filter(|goal| scopes.contains(&goal.scope))
            .ok_or_else(|| DaemonHostError::Worker(GOAL_NOT_VISIBLE.to_owned()))
    }
}

/// 当前调用可见的 Goal 作用域集合。
///
/// 缺省 policy 是旧客户端的本地/私聊边界；Workspace 边界则只能读取绑定的
/// Workspace，不能探测或修改 Global Work Ledger。
fn visible_scopes(
    workspace: Option<&WorkspaceRecord>,
    disclosure_policy: Option<DisclosurePolicy>,
) -> Result<Vec<ScopeRef>, DaemonHostError> {
    let mut scopes = Vec::new();
    if disclosure_policy.is_none_or(|policy| policy == DisclosurePolicy::PrivateOnly) {
        scopes.push(ScopeRef::global());
    }
    if let Some(workspace) = workspace {
        scopes.push(
            ScopeRef::workspace(workspace.workspace_id.clone())
                .map_err(|error| DaemonHostError::Worker(error.to_string()))?,
        );
    }
    Ok(scopes)
}

/// 把列表查询的 scope 语义解析为具体可见作用域。
fn visible_scope_set(
    scope: &axon_domain::GoalQueryScope,
    workspace: Option<&WorkspaceRecord>,
    disclosure_policy: Option<DisclosurePolicy>,
) -> Result<Vec<ScopeRef>, DaemonHostError> {
    match scope {
        axon_domain::GoalQueryScope::Global => {
            if disclosure_policy.is_some_and(|policy| policy != DisclosurePolicy::PrivateOnly) {
                return Err(DaemonHostError::Worker(GOAL_GLOBAL_NOT_ALLOWED.to_owned()));
            }
            Ok(vec![ScopeRef::global()])
        }
        axon_domain::GoalQueryScope::CurrentWorkspace => {
            let workspace = workspace.ok_or_else(|| {
                DaemonHostError::Worker("查询当前 Workspace Goal 需要绑定 Workspace".to_owned())
            })?;
            Ok(vec![
                ScopeRef::workspace(workspace.workspace_id.clone())
                    .map_err(|error| DaemonHostError::Worker(error.to_string()))?,
            ])
        }
        axon_domain::GoalQueryScope::CurrentContext | axon_domain::GoalQueryScope::AllVisible => {
            visible_scopes(workspace, disclosure_policy)
        }
    }
}

/// 构造 GoalCreated Event；payload 只暴露 Goal 身份与目标，不泄露内部执行细节。
fn goal_created_event(
    scope: &ScopeRef,
    command: &GoalCreateCommand,
    goal_id: &GoalId,
    now_ms: u64,
) -> Result<JarvisEvent, DaemonHostError> {
    let event_key = format!("{GOAL_EVENT_ID_PREFIX}{goal_id}");
    Ok(JarvisEvent {
        event_id: JarvisEventId::new(event_key.clone())
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?,
        sequence: 0,
        event_type: GOAL_CREATED_EVENT_TYPE.to_owned(),
        idempotency_key: event_key,
        cause_event_id: None,
        payload_json: serde_json::to_string(&json!({
            "goal_id": goal_id.as_str(),
            "scope": scope,
            "title": command.title,
            "create_execution": command.create_execution,
        }))
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?,
        occurred_at_ms: now_ms,
    })
}

fn goal_started_event(
    goal_id: &GoalId,
    revision: u64,
    now_ms: u64,
) -> Result<JarvisEvent, DaemonHostError> {
    let event_key = format!("goal.started:{}:{}", goal_id.as_str(), revision);
    Ok(JarvisEvent {
        event_id: JarvisEventId::new(format!("event-{event_key}"))
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?,
        sequence: 0,
        event_type: "goal.started".to_owned(),
        idempotency_key: event_key,
        cause_event_id: None,
        payload_json: serde_json::json!({
            "goal_id": goal_id.as_str(),
            "revision": revision,
        })
        .to_string(),
        occurred_at_ms: now_ms,
    })
}

fn goal_receipt(tool: &str, goal: &axon_domain::GoalRecord, created: bool) -> Value {
    json!({
        "tool": tool,
        "goalId": goal.goal_id,
        "status": goal.status,
        "revision": goal.revision,
        "created": created,
    })
}

fn goal_service_error(error: impl std::fmt::Display) -> DaemonHostError {
    DaemonHostError::Worker(error.to_string())
}

#[cfg(test)]
#[path = "daemon_host_goal_tools_tests.rs"]
mod tests;
