//! 目标执行的 Interaction 投递：目标轮与用户轮共用同一条主 turn 管道。
//!
//! 目标执行没有专用后台入口：启动目标、更新目标与失败重投都只是往目标会话里提交一条
//! Interaction。领取、执行、逐条投影、指标与终态全部由主 turn 承担，所以这里只负责把
//! 目标契约翻译成一条入站事实，不碰执行。

use super::daemon_host_goal_tools_ledger::goal_service_error;
use super::goal_execution;
use super::{DaemonHost, DaemonHostError};
use crate::ports::ProcessIdentity;
use axon_domain::{
    AGENT_DISPATCH_SOURCE_CHANNEL, AudienceContext, AudienceKind, DisclosurePolicy, GoalRecord,
    GoalStatus, InteractionId, InteractionRecord, InteractionSource, ReplyRoute,
};
use axon_session::{GoalExecutionSpec, GoalSessionRequest, RuntimeStore};
use axon_session_runtime::{EventPublisher, ORIGIN_CHANNEL_GOAL, RuntimeClock, TurnExecutor};
use serde_json::Value;

/// 目标执行 Interaction 的来源消息前缀。
const GOAL_EXECUTION_SOURCE_PREFIX: &str = "goal-execution:";
/// 目标更新 Interaction 的来源消息前缀。
const GOAL_UPDATE_SOURCE_PREFIX: &str = "goal-update:";
/// 目标执行 payload 里的提示字段。
const PAYLOAD_PROMPT_KEY: &str = "prompt";

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 更新之后把已收尾的目标放回执行态；取消与仍在执行的保持原状。
    pub(super) fn reactivate_goal_after_update(
        &self,
        goal: &axon_domain::GoalRecord,
        now_ms: u64,
    ) -> Result<axon_domain::GoalRecord, DaemonHostError> {
        if !matches!(
            goal.status,
            axon_domain::GoalStatus::Completed | axon_domain::GoalStatus::Failed
        ) {
            return Ok(goal.clone());
        }
        let expected_revision = goal.revision;
        let mut reactivated = goal.clone();
        reactivated
            .transition(axon_domain::GoalStatus::Active, now_ms)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        self.store
            .update_goal(&reactivated, expected_revision)
            .map_err(goal_service_error)?;
        Ok(reactivated)
    }

    /// 把一次目标执行提交成一条主 turn Interaction。
    ///
    /// 目标执行与用户输入从此只剩一处区别：这条 Interaction 由 Daemon 在启动目标时写，
    /// 而不是由外部渠道提交。之后领取、执行、投影、指标与终态全部复用主 turn 那条管道，
    /// 事件因此天然带 interaction 归属，界面不必为后台执行另写一套渲染分支。
    pub(super) fn submit_goal_execution_interaction(
        &self,
        goal: &GoalRecord,
        execution: &GoalExecutionSpec,
        now_ms: u64,
    ) -> Result<(), DaemonHostError> {
        let payload = execution_payload(&execution.payload_json)?;
        let prompt = payload_prompt(&payload)?;
        let audience = payload_audience(&payload);
        // 会话执行标识与首轮 interaction 同源：一条目标执行在界面与账本里始终是同一枚
        // 标识；重试轮另立主键，否则 inbox 主键会把新一轮挡在门外。
        let interaction_id = InteractionId::new(if execution.attempt <= 1 {
            execution.execution_id.as_str().to_owned()
        } else {
            format!("{}-r{}", execution.execution_id.as_str(), execution.attempt)
        })
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let source_id = format!(
            "{GOAL_EXECUTION_SOURCE_PREFIX}{}:{}",
            goal.goal_id.as_str(),
            execution.attempt
        );
        self.append_goal_interaction(
            goal,
            &execution.session_id,
            interaction_id,
            source_id,
            audience,
            prompt,
            now_ms,
        )
    }

    /// 目标更新后向目标会话投递一笔新要求。
    ///
    /// 更新只改契约、不重启执行：目标正在跑时它作为下一轮的要求排队，目标没在跑时它就是
    /// 重新唤醒执行的那一轮。两种情形都只是往会话里多一条 Interaction。
    pub(super) fn submit_goal_update_interaction(
        &self,
        goal: &GoalRecord,
        workspace_id: Option<&str>,
        workspace_root: &str,
        disclosure_policy: Option<DisclosurePolicy>,
        now_ms: u64,
    ) -> Result<bool, DaemonHostError> {
        // 除了已取消，任何状态都该接收新想法：刚收尾的目标也要能被唤醒继续执行。
        if goal.status == GoalStatus::Cancelled {
            return Ok(false);
        }
        let session_id = self
            .store
            .resolve_goal_session(&GoalSessionRequest {
                goal_id: goal.goal_id.as_str().to_owned(),
                workspace_id: workspace_id.map(str::to_owned),
                cwd: workspace_root.to_owned(),
                now_ms,
            })
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?
            .session_id;
        let prompt = goal_execution::prompt(
            &goal.title,
            &goal.plan.objective,
            &goal.plan.steps,
            &goal.acceptance,
            goal.plan.write_policy,
        );
        let interaction_id = axon_session::new_interaction_id();
        let source_id = format!(
            "{GOAL_UPDATE_SOURCE_PREFIX}{}:{}",
            goal.goal_id.as_str(),
            goal.revision
        );
        self.append_goal_interaction(
            goal,
            &session_id,
            interaction_id,
            source_id,
            audience_for(disclosure_policy),
            prompt,
            now_ms,
        )?;
        Ok(true)
    }

    /// 失败重投：把到期重试的执行重新提交成一条新 Interaction。
    pub(super) fn resubmit_goal_execution(
        &self,
        goal: &GoalRecord,
        execution: &axon_domain::ExecutionRecord,
        now_ms: u64,
    ) -> Result<(), DaemonHostError> {
        let payload = execution_payload(&execution.payload_json)?;
        let prompt = payload_prompt(&payload)?;
        let audience = payload_audience(&payload);
        let session_id = execution
            .session_id
            .clone()
            .ok_or_else(|| DaemonHostError::Worker("重投执行缺少会话绑定".to_owned()))?;
        let interaction_id = InteractionId::new(format!(
            "{}-r{}",
            execution.execution_id.as_str(),
            execution.attempt
        ))
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let source_id = format!(
            "{GOAL_EXECUTION_SOURCE_PREFIX}{}:{}",
            goal.goal_id.as_str(),
            execution.attempt
        );
        self.append_goal_interaction(
            goal,
            &session_id,
            interaction_id,
            source_id,
            audience,
            prompt,
            now_ms,
        )
    }

    /// 退避到期的目标执行重新投递成一条 Interaction。
    ///
    /// 目标轮的重试不再依赖 Worker 重新领取：退避到期就是再提交一条 Interaction，
    /// 与首次启动走完全相同的领取与执行路径。返回本次重新投递的条数。
    pub(super) fn redispatch_goal_executions(&self, now_ms: u64, limit: u32) -> u32 {
        let Ok(due) = self.store.due_execution_retries(now_ms, limit) else {
            return 0;
        };
        let mut redispatched = 0_u32;
        for execution in due {
            let Ok(Some(goal)) = self.store.get_goal(&execution.goal_id) else {
                continue;
            };
            if goal.status == GoalStatus::Cancelled {
                continue;
            }
            if self
                .resubmit_goal_execution(&goal, &execution, now_ms)
                .is_ok()
            {
                redispatched = redispatched.saturating_add(1);
            }
        }
        redispatched
    }

    /// 写入一条目标 Interaction：目标轮唯一的入站事实。
    fn append_goal_interaction(
        &self,
        goal: &GoalRecord,
        session_id: &axon_domain::DaemonSessionId,
        interaction_id: InteractionId,
        source_id: String,
        audience: AudienceContext,
        prompt: String,
        now_ms: u64,
    ) -> Result<(), DaemonHostError> {
        let source = InteractionSource::new(AGENT_DISPATCH_SOURCE_CHANNEL, source_id)
            .and_then(|source| source.with_client(ORIGIN_CHANNEL_GOAL))
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let route = ReplyRoute::new(AGENT_DISPATCH_SOURCE_CHANNEL, session_id.as_str())
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let interaction = InteractionRecord::new(
            session_id.clone(),
            interaction_id,
            source,
            goal.scope.clone(),
            audience,
            route,
            prompt,
            now_ms,
        )
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        self.store
            .append(&interaction)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        Ok(())
    }
}

/// 解析目标执行 payload。
fn execution_payload(payload_json: &str) -> Result<Value, DaemonHostError> {
    serde_json::from_str(payload_json)
        .map_err(|error| DaemonHostError::Worker(format!("目标执行 payload 解析失败：{error}")))
}

/// 取出本轮交给模型的完整提示。
fn payload_prompt(payload: &Value) -> Result<String, DaemonHostError> {
    payload
        .get(PAYLOAD_PROMPT_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| DaemonHostError::Worker("目标执行缺少 prompt".to_owned()))
}

/// 目标执行 payload 里携带的受众；缺失时回落到私聊私有边界。
fn payload_audience(payload: &Value) -> AudienceContext {
    payload
        .get("transportContext")
        .and_then(|context| context.get("audience"))
        .and_then(|value| serde_json::from_value::<AudienceContext>(value.clone()).ok())
        .unwrap_or_else(|| audience_for(None))
}

/// 按披露策略给受众：群聊覆盖 Workspace，其余一律私聊私有。
fn audience_for(disclosure_policy: Option<DisclosurePolicy>) -> AudienceContext {
    match disclosure_policy {
        Some(DisclosurePolicy::Workspace) => {
            AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace)
        }
        _ => AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
    }
}
