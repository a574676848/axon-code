//! JARVIS Goal 查询应用服务。

use std::fmt;

use axon_domain::{
    GoalActivityView, GoalDetailQuery, GoalListQuery, GoalProgress, GoalRecord, JarvisEvent,
    ScopeRef,
};

use crate::ports::{
    ExecutionStore, GoalPage, GoalStore, GoalStoreError, LedgerEventStore, ResultStore,
};

const MAX_DETAIL_ITEMS: u32 = 256;

/// Goal 查询服务错误。
#[derive(Debug)]
pub enum GoalQueryServiceError {
    /// Goal 存储错误。
    Goal(GoalStoreError),
    /// Execution、Result 或 Event 存储错误。
    Storage(String),
}

impl fmt::Display for GoalQueryServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Goal(error) => write!(formatter, "Goal 查询失败：{error}"),
            Self::Storage(error) => write!(formatter, "Goal Activity 查询失败：{error}"),
        }
    }
}

impl std::error::Error for GoalQueryServiceError {}

/// 直接读取 Work Ledger 事实源的 Goal 查询服务。
#[derive(Debug, Clone)]
pub struct GoalQueryService<S> {
    store: S,
}

impl<S> GoalQueryService<S>
where
    S: GoalStore + ExecutionStore + ResultStore + LedgerEventStore,
{
    /// 创建查询服务。
    #[must_use]
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// 查询 Goal 分页摘要；scopes 为调用方可见的作用域集合。
    pub fn list(
        &self,
        scopes: &[ScopeRef],
        query: &GoalListQuery,
    ) -> Result<GoalPage, GoalQueryServiceError> {
        self.store
            .list_goals_in_scopes(scopes, query)
            .map_err(GoalQueryServiceError::Goal)
    }

    /// 聚合指定 Goal 的最新 Activity。
    pub fn get(
        &self,
        scopes: &[ScopeRef],
        query: &GoalDetailQuery,
    ) -> Result<Option<GoalActivityView>, GoalQueryServiceError> {
        let Some(goal) = self
            .store
            .get_goal(&query.goal_id)
            .map_err(GoalQueryServiceError::Goal)?
        else {
            return Ok(None);
        };
        if !scopes.contains(&goal.scope) {
            return Ok(None);
        }
        let executions = self
            .store
            .list_executions_for_goal(&goal.goal_id, query.limit)
            .map_err(|error| GoalQueryServiceError::Storage(error.to_string()))?;
        let latest_execution = executions.first().cloned();
        let latest_result = latest_execution
            .as_ref()
            .map(|execution| {
                self.store
                    .result_for_execution(&execution.execution_id)
                    .map_err(|error| GoalQueryServiceError::Storage(error.to_string()))
            })
            .transpose()?
            .flatten();
        let recent_events = if query.include_events {
            self.events_for_goal(&goal, query.limit)?
        } else {
            Vec::new()
        };
        Ok(Some(GoalActivityView {
            progress: progress_from_goal(&goal),
            goal,
            latest_execution,
            recent_executions: if query.include_executions {
                executions
            } else {
                Vec::new()
            },
            recent_events,
            latest_result,
            pending_decision: None,
            next_cursor: None,
        }))
    }

    fn events_for_goal(
        &self,
        goal: &GoalRecord,
        limit: u32,
    ) -> Result<Vec<JarvisEvent>, GoalQueryServiceError> {
        let bounded_limit = limit.min(MAX_DETAIL_ITEMS);
        self.store
            .list_events_for_goal(&goal.goal_id, bounded_limit)
            .map_err(|error| GoalQueryServiceError::Storage(error.to_string()))
            .map(|mut events| {
                events.sort_by(|left, right| {
                    left.occurred_at_ms
                        .cmp(&right.occurred_at_ms)
                        .then_with(|| left.sequence.cmp(&right.sequence))
                        .then_with(|| left.event_id.as_str().cmp(right.event_id.as_str()))
                });
                events
            })
    }
}

fn progress_from_goal(goal: &GoalRecord) -> Option<GoalProgress> {
    if let Some(progress) = &goal.progress {
        return Some(progress.clone());
    }
    if goal.plan.steps.is_empty() {
        return None;
    }
    let completed = if goal.status == axon_domain::GoalStatus::Completed {
        goal.plan.steps.len() as u32
    } else {
        0
    };
    Some(GoalProgress {
        completed_step_count: completed,
        total_step_count: goal.plan.steps.len() as u32,
        current_step: goal
            .next_step
            .clone()
            .or_else(|| goal.plan.steps.get(completed as usize).cloned()),
        summary: goal.waiting_reason.clone(),
        updated_at_ms: goal.updated_at_ms,
    })
}
