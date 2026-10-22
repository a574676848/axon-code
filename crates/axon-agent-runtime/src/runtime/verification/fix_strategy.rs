//! Fix Strategy Diversity - 修复策略多样性层。
//!
//! 参考 codex-rs 设计，实现：
//! - Fork modes（FullHistory / LastNTurns）
//! - Rollout item filtering strategies
//! - 自动策略选择

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Fork 模式 - 控制历史记录的保留策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ForkMode {
    /// 保留完整历史记录
    #[default]
    FullHistory,
    /// 仅保留最近 N 轮对话
    LastNTurns(u32),
}

impl ForkMode {
    /// 获取保留的轮数。
    #[must_use]
    pub fn turn_limit(&self) -> Option<u32> {
        match self {
            Self::FullHistory => None,
            Self::LastNTurns(n) => Some(*n),
        }
    }

    /// 判断是否为完整历史模式。
    #[must_use]
    pub fn is_full_history(&self) -> bool {
        matches!(self, Self::FullHistory)
    }
}

/// Rollout 条目类型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RolloutItemKind {
    /// 尝试修复
    AttemptFix,
    /// 并行探索多个分支
    ExploreBranch,
    /// 简化问题
    Simplify,
    /// 寻求外部帮助
    ExternalHelp,
    /// 重试
    Retry,
}

/// Rollout 条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutItem {
    /// 全局唯一 ID
    pub id: u64,
    /// 条目类型
    pub kind: RolloutItemKind,
    /// 描述
    pub description: String,
    /// 关联的错误信息
    pub error_context: Option<String>,
    /// 关联的文件路径
    pub file_path: Option<String>,
    /// 创建时间戳
    pub created_at_ms: u64,
    /// 状态
    pub status: RolloutStatus,
    /// 成功率（0.0 - 1.0）
    pub success_rate: f32,
    /// 执行时长（毫秒）
    pub duration_ms: Option<u64>,
}

/// Rollout 条目状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RolloutStatus {
    /// 待执行
    #[default]
    Pending,
    /// 执行中
    Running,
    /// 成功
    Succeeded,
    /// 失败
    Failed,
    /// 跳过
    Skipped,
}

/// Rollout 过滤器策略。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum RolloutFilterStrategy {
    /// 无过滤，保留所有条目
    #[default]
    NoFilter,
    /// 仅保留成功条目
    KeepSuccessful,
    /// 仅保留最近 N 条
    KeepRecent(u32),
    /// 成功率阈值（保留 >= threshold 的条目）
    SuccessRateThreshold(f32),
    /// 组合过滤：成功 + 最近
    SuccessfulAndRecent(u32),
}

impl RolloutFilterStrategy {
    /// 应用过滤策略到条目列表。
    #[must_use]
    pub fn apply(&self, items: &[RolloutItem]) -> Vec<RolloutItem> {
        match self {
            Self::NoFilter => items.to_vec(),

            Self::KeepSuccessful => items
                .iter()
                .filter(|i| matches!(i.status, RolloutStatus::Succeeded))
                .cloned()
                .collect(),

            Self::KeepRecent(n) => {
                let mut sorted = items.to_vec();
                sorted.sort_by_key(|item| std::cmp::Reverse(item.created_at_ms));
                sorted.truncate(*n as usize);
                sorted
            }

            Self::SuccessRateThreshold(threshold) => items
                .iter()
                .filter(|i| i.success_rate >= *threshold)
                .cloned()
                .collect(),

            Self::SuccessfulAndRecent(n) => {
                let successful: Vec<_> = items
                    .iter()
                    .filter(|i| matches!(i.status, RolloutStatus::Succeeded))
                    .cloned()
                    .collect();
                let mut sorted = successful;
                sorted.sort_by_key(|item| std::cmp::Reverse(item.created_at_ms));
                sorted.truncate(*n as usize);
                sorted
            }
        }
    }
}

/// Rollout 上下文。
#[derive(Debug, Clone)]
pub struct RolloutContext {
    /// 会话 ID
    pub session_id: String,
    /// 当前线程 ID
    pub thread_id: u64,
    /// Fork 模式
    pub fork_mode: ForkMode,
    /// 过滤器策略
    pub filter_strategy: RolloutFilterStrategy,
    /// 最大并发 rollouts
    pub max_concurrent: usize,
}

impl Default for RolloutContext {
    fn default() -> Self {
        Self {
            session_id: "default".into(),
            thread_id: 1,
            fork_mode: ForkMode::default(),
            filter_strategy: RolloutFilterStrategy::default(),
            max_concurrent: 4,
        }
    }
}

impl RolloutContext {
    /// 构造新上下文。
    pub fn new(session_id: impl Into<String>, thread_id: u64) -> Self {
        Self {
            session_id: session_id.into(),
            thread_id,
            ..Default::default()
        }
    }

    /// 设置 Fork 模式。
    #[must_use]
    pub fn with_fork_mode(mut self, mode: ForkMode) -> Self {
        self.fork_mode = mode;
        self
    }

    /// 设置过滤策略。
    #[must_use]
    pub fn with_filter_strategy(mut self, strategy: RolloutFilterStrategy) -> Self {
        self.filter_strategy = strategy;
        self
    }

    /// 设置最大并发数。
    #[must_use]
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max;
        self
    }

    /// 计算保留的历史轮数。
    #[must_use]
    pub fn retained_turns(&self, total_turns: u32) -> u32 {
        match self.fork_mode {
            ForkMode::FullHistory => total_turns,
            ForkMode::LastNTurns(n) => total_turns.saturating_sub(total_turns.saturating_sub(n)),
        }
    }
}

/// Rollout Manager - 管理修复策略的执行与历史。
#[derive(Debug)]
pub struct RolloutManager {
    /// 所有 rollout 条目
    items: Arc<RwLock<Vec<RolloutItem>>>,
    /// 下一个 ID
    next_id: Arc<RwLock<u64>>,
    /// 最大条目数
    max_items: usize,
}

impl Default for RolloutManager {
    fn default() -> Self {
        Self::new(256)
    }
}

impl RolloutManager {
    /// 创建新实例。
    #[must_use]
    pub fn new(max_items: usize) -> Self {
        Self {
            items: Arc::new(RwLock::new(Vec::new())),
            next_id: Arc::new(RwLock::new(1)),
            max_items,
        }
    }

    /// 添加 rollout 条目。
    pub async fn add(&self, item: RolloutItem) {
        let mut items = self.items.write().await;
        if items.len() >= self.max_items {
            items.remove(0);
        }
        items.push(item);
    }

    /// 创建新条目并添加。
    pub async fn create_item(
        &self,
        kind: RolloutItemKind,
        description: impl Into<String>,
        error_context: Option<String>,
        file_path: Option<String>,
    ) -> RolloutItem {
        let mut next_id = self.next_id.write().await;
        let id = *next_id;
        *next_id += 1;
        drop(next_id);

        let item = RolloutItem {
            id,
            kind,
            description: description.into(),
            error_context,
            file_path,
            created_at_ms: timestamp_ms(),
            status: RolloutStatus::Pending,
            success_rate: 0.0,
            duration_ms: None,
        };

        self.add(item.clone()).await;
        item
    }

    /// 更新条目状态。
    pub async fn update_status(
        &self,
        id: u64,
        status: RolloutStatus,
        success_rate: Option<f32>,
        duration_ms: Option<u64>,
    ) {
        let mut items = self.items.write().await;
        if let Some(item) = items.iter_mut().find(|i| i.id == id) {
            item.status = status;
            if let Some(sr) = success_rate {
                item.success_rate = sr;
            }
            if let Some(dm) = duration_ms {
                item.duration_ms = Some(dm);
            }
        }
    }

    /// 获取所有条目（应用过滤器）。
    pub async fn get_items(&self, strategy: RolloutFilterStrategy) -> Vec<RolloutItem> {
        let items = self.items.read().await;
        strategy.apply(&items)
    }

    /// 获取最近的 N 条目。
    pub async fn get_recent(&self, n: u32) -> Vec<RolloutItem> {
        let items = self.items.read().await;
        let mut sorted = items.clone();
        sorted.sort_by_key(|item| std::cmp::Reverse(item.created_at_ms));
        sorted.truncate(n as usize);
        sorted
    }

    /// 获取成功的条目。
    pub async fn get_successful(&self) -> Vec<RolloutItem> {
        let items = self.items.read().await;
        items
            .iter()
            .filter(|i| matches!(i.status, RolloutStatus::Succeeded))
            .cloned()
            .collect()
    }

    /// 清空所有条目。
    pub async fn clear(&self) {
        let mut items = self.items.write().await;
        items.clear();
    }

    /// 统计信息。
    pub async fn stats(&self) -> RolloutStats {
        let items = self.items.read().await;
        let total = items.len();
        let successful = items
            .iter()
            .filter(|i| matches!(i.status, RolloutStatus::Succeeded))
            .count();
        let failed = items
            .iter()
            .filter(|i| matches!(i.status, RolloutStatus::Failed))
            .count();
        let pending = items
            .iter()
            .filter(|i| matches!(i.status, RolloutStatus::Pending))
            .count();
        let avg_success_rate = if total > 0 {
            items.iter().map(|i| f64::from(i.success_rate)).sum::<f64>() / total as f64
        } else {
            0.0
        };

        RolloutStats {
            total,
            successful,
            failed,
            pending,
            avg_success_rate: avg_success_rate as f32,
        }
    }
}

/// Rollout 统计信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutStats {
    pub total: usize,
    pub successful: usize,
    pub failed: usize,
    pub pending: usize,
    pub avg_success_rate: f32,
}

/// 获取当前时间戳（毫秒）。
fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_keep_recent_and_context_retained_turns() {
        let mut items = vec![
            make_item(1, RolloutStatus::Succeeded),
            make_item(2, RolloutStatus::Failed),
            make_item(3, RolloutStatus::Succeeded),
        ];
        items[0].created_at_ms = 1000;
        items[1].created_at_ms = 2000;
        items[2].created_at_ms = 3000;

        let filtered = RolloutFilterStrategy::KeepRecent(2).apply(&items);
        assert_eq!(filtered.len(), 2);

        let ctx = RolloutContext::new("s1", 1).with_fork_mode(ForkMode::LastNTurns(5));
        assert_eq!(ctx.retained_turns(10), 5);
        assert_eq!(ctx.retained_turns(3), 3);
    }

    fn make_item(id: u64, status: RolloutStatus) -> RolloutItem {
        RolloutItem {
            id,
            kind: RolloutItemKind::AttemptFix,
            description: format!("item {id}"),
            error_context: None,
            file_path: None,
            created_at_ms: 1000,
            status,
            success_rate: 0.5,
            duration_ms: None,
        }
    }
}
