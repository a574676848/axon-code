//! Work Ledger Goal 的持久化端口。

use axon_domain::{GoalId, GoalListQuery, GoalRecord, JarvisEvent, ScopeRef};
use std::fmt;

/// Goal 存储错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalStoreError {
    /// 记录不存在。
    NotFound(GoalId),
    /// revision 不匹配或状态冲突。
    Conflict(String),
    /// 存储失败。
    Storage(String),
}

impl fmt::Display for GoalStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(goal_id) => write!(formatter, "Goal 不存在：{goal_id}"),
            Self::Conflict(detail) => write!(formatter, "Goal 冲突：{detail}"),
            Self::Storage(detail) => write!(formatter, "Goal 存储失败：{detail}"),
        }
    }
}

impl std::error::Error for GoalStoreError {}

/// Goal 列表分页结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalPage {
    /// 当前页 Goal。
    pub goals: Vec<GoalRecord>,
    /// 没有下一页时为空。
    pub next_cursor: Option<String>,
}

/// Goal 持久化端口。
pub trait GoalStore: Send + Sync {
    /// 幂等创建 Goal；相同 ID 且内容一致时返回 false。
    fn create_goal(&self, goal: &GoalRecord) -> Result<bool, GoalStoreError>;

    /// 读取 Goal。
    fn get_goal(&self, goal_id: &GoalId) -> Result<Option<GoalRecord>, GoalStoreError>;

    /// 按更新时间读取非终态 Goal。
    fn list_active_goals(&self, limit: u32) -> Result<Vec<GoalRecord>, GoalStoreError>;

    /// 按作用域读取非终态 Goal；默认实现保留给轻量内存适配器。
    fn list_active_goals_for_scope(
        &self,
        scope: &ScopeRef,
        limit: u32,
    ) -> Result<Vec<GoalRecord>, GoalStoreError> {
        Ok(self
            .list_active_goals(limit)?
            .into_iter()
            .filter(|goal| &goal.scope == scope)
            .collect())
    }

    /// 使用 expected revision 原子更新 Goal。
    fn update_goal(&self, goal: &GoalRecord, expected_revision: u64) -> Result<(), GoalStoreError>;

    /// 持久化取消请求并阻止未领取 Execution。
    fn request_cancellation(
        &self,
        goal_id: &GoalId,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), GoalStoreError> {
        let _ = (goal_id, reason, now_ms);
        Ok(())
    }

    /// 在同一事务内更新 Goal 并持久化取消请求。
    fn cancel_goal(
        &self,
        goal: &GoalRecord,
        expected_revision: u64,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), GoalStoreError> {
        self.update_goal(goal, expected_revision)?;
        self.request_cancellation(&goal.goal_id, reason, now_ms)
    }

    /// 按结构化状态和稳定游标读取 Goal。
    fn list_goals(
        &self,
        scope: &ScopeRef,
        query: &GoalListQuery,
    ) -> Result<GoalPage, GoalStoreError> {
        let mut goals = self.list_active_goals_for_scope(scope, query.limit)?;
        if !query.statuses.is_empty() {
            goals.retain(|goal| query.statuses.contains(&goal.status));
        }
        Ok(GoalPage {
            goals,
            next_cursor: None,
        })
    }

    /// 按多个作用域读取 Goal；游标由实现绑定作用域集合与过滤器。
    ///
    /// 默认实现逐个作用域读取后合并，仅保留稳定排序与单页截断，不生成游标；
    /// 具备结构化索引的实现应覆盖它并提供跨作用域游标。
    fn list_goals_in_scopes(
        &self,
        scopes: &[ScopeRef],
        query: &GoalListQuery,
    ) -> Result<GoalPage, GoalStoreError> {
        if let [scope] = scopes {
            return self.list_goals(scope, query);
        }
        let mut goals = Vec::new();
        for scope in scopes {
            goals.extend(self.list_active_goals_for_scope(scope, query.limit)?);
        }
        if !query.statuses.is_empty() {
            goals.retain(|goal| query.statuses.contains(&goal.status));
        }
        goals.sort_by(|left, right| {
            left.updated_at_ms
                .cmp(&right.updated_at_ms)
                .then_with(|| left.goal_id.as_str().cmp(right.goal_id.as_str()))
        });
        goals.truncate(query.limit as usize);
        Ok(GoalPage {
            goals,
            next_cursor: None,
        })
    }
}

/// 在同一事务内更新 Goal 并追加审计 Event。
pub trait GoalMutationStore: GoalStore {
    /// 使用 expected revision 原子更新 Goal 和 Event。
    fn update_goal_with_event(
        &self,
        goal: &GoalRecord,
        expected_revision: u64,
        event: &JarvisEvent,
    ) -> Result<(), GoalStoreError>;
}
