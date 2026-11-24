//! Goal 工具更新在目标会话里留下的轮次记录。
//!
//! 来源会话每调用一次 Goal 工具更新目标（create / update / start / cancel），
//! 目标会话里就要多一条 inbox 记录，把这次更新记成该会话的一轮。这样目标看板打开
//! 执行会话时，看到的是「谁在什么时候改了目标」的完整时间线，而不是只有执行轮。
//!
//! 记录走派发渠道（`agent`）：它没有 provider 轮，由这里直接落成已收口轮次，
//! 因此不会被 Primary Turn Worker 领取重跑。

use axon_domain::{
    AGENT_DISPATCH_SOURCE_CHANNEL, AudienceContext, AudienceKind, DisclosurePolicy, GoalRecord,
    GoalStatus, InteractionId, InteractionRecord, InteractionSource, ReplyRoute,
};
use axon_session::{DispatchTerminalKind, RuntimeStore};
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};

use super::{DaemonHost, DaemonHostError};

/// 工具轮次的来源客户端标识：与会话卡片上的来源标签同源。
const GOAL_TOOL_CLIENT: &str = "goal";
/// 工具轮次的来源消息标识前缀：同一目标同一次工具调用只落一条。
const GOAL_TOOL_SOURCE_PREFIX: &str = "goal-tool:";

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: crate::ports::ProcessIdentity + Clone + 'static,
{
    /// 记录一次 Goal 工具更新；落库失败只告警，不影响已经生效的目标变更。
    ///
    /// `workspace` 缺省表示全局目标：会话按目标标识解析，没有 Workspace 也能落到同一条。
    pub(super) fn note_goal_turn(
        &self,
        goal: &GoalRecord,
        label: &str,
        idempotency_key: &str,
        workspace: Option<&axon_session::WorkspaceRecord>,
        now_ms: u64,
    ) {
        let workspace_id = workspace.map(|item| item.workspace_id.as_str());
        let cwd = workspace.map_or("", |item| item.workspace_root.as_str());
        self.note_goal_tool_turn(goal, label, idempotency_key, workspace_id, cwd, now_ms);
    }

    /// 记录一次 Goal 工具更新；落库失败只告警，不影响已经生效的目标变更。
    pub(super) fn note_goal_tool_turn(
        &self,
        goal: &GoalRecord,
        label: &str,
        idempotency_key: &str,
        workspace_id: Option<&str>,
        cwd: &str,
        now_ms: u64,
    ) {
        if let Err(error) =
            self.record_goal_tool_turn(goal, label, idempotency_key, workspace_id, cwd, now_ms)
        {
            axon_logger::warning(&format!(
                "Goal 工具轮次落库失败: goal={}, label={label}, error={error}",
                goal.goal_id.as_str()
            ));
        }
    }

    /// 把一次 Goal 工具更新记成目标会话里的一轮（已收口）。
    fn record_goal_tool_turn(
        &self,
        goal: &GoalRecord,
        label: &str,
        idempotency_key: &str,
        workspace_id: Option<&str>,
        cwd: &str,
        now_ms: u64,
    ) -> Result<(), DaemonHostError> {
        let key = idempotency_key.trim();
        if key.is_empty() {
            return Err(DaemonHostError::Worker(
                "Goal 工具轮次缺少幂等键".to_owned(),
            ));
        }
        let session_id = self
            .store
            .resolve_goal_session(&axon_session::GoalSessionRequest {
                goal_id: goal.goal_id.as_str().to_owned(),
                workspace_id: workspace_id.map(str::to_owned),
                cwd: cwd.to_owned(),
                now_ms,
            })
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?
            .session_id;
        let source_id = format!("{GOAL_TOOL_SOURCE_PREFIX}{}:{key}", goal.goal_id.as_str());
        let source = InteractionSource::new(AGENT_DISPATCH_SOURCE_CHANNEL, source_id.clone())
            .and_then(|source| source.with_client(GOAL_TOOL_CLIENT))
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let route = ReplyRoute::new(AGENT_DISPATCH_SOURCE_CHANNEL, session_id.as_str())
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let audience = AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly);
        let interaction_id = InteractionId::new(source_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let interaction = InteractionRecord::new(
            session_id,
            interaction_id,
            source,
            goal.scope.clone(),
            audience,
            route,
            format!("{label}：{}", goal.title),
            now_ms,
        )
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        // 落库即收口：这一轮没有 provider 参与，界面看到的就是一条已完成的记录。
        self.store
            .append(&interaction)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        self.store
            .settle_dispatch(
                &interaction.interaction_id,
                DispatchTerminalKind::Understood,
                now_ms,
            )
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        Ok(())
    }

    /// 取消后的运行侧联动：在目标会话上写一条取消标记，让正在跑的那一轮收口。
    ///
    /// 取消只能由来源会话调用 Goal 工具发起，因此这里是取消标记唯一的写入点；
    /// 目标本来就没在跑时被标记的轮次数为 0，属于正常情况。
    pub(super) fn mark_goal_session_cancelled(
        &self,
        goal: &GoalRecord,
        reason: &str,
        workspace: Option<&axon_session::WorkspaceRecord>,
        now_ms: u64,
    ) {
        let workspace_id = workspace.map(|item| item.workspace_id.as_str());
        let cwd = workspace.map_or("", |item| item.workspace_root.as_str());
        let request = axon_session::GoalSessionRequest {
            goal_id: goal.goal_id.as_str().to_owned(),
            workspace_id: workspace_id.map(str::to_owned),
            cwd: cwd.to_owned(),
            now_ms,
        };
        let Ok(session) = self.store.resolve_goal_session(&request) else {
            axon_logger::warning(&format!(
                "取消标记跳过：无法解析目标会话 goal={}",
                goal.goal_id.as_str()
            ));
            return;
        };
        if let Err(error) =
            self.store
                .request_session_cancellation(&session.session_id, reason, now_ms)
        {
            axon_logger::warning(&format!(
                "取消标记写入失败: goal={}, error={error}",
                goal.goal_id.as_str()
            ));
        }
    }

    /// 工具轮次的措辞：按更新后的目标状态给一句人话标题。
    pub(super) fn goal_tool_label(status: GoalStatus) -> &'static str {
        match status {
            GoalStatus::Cancelled => "目标已取消",
            GoalStatus::Completed => "目标已完成",
            GoalStatus::Failed => "目标已失败",
            GoalStatus::Active | GoalStatus::Paused => "目标已启动",
            GoalStatus::Proposed => "目标已创建",
            GoalStatus::Waiting => "目标已更新",
        }
    }
}
