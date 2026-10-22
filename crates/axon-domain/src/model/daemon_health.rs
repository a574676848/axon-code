//! Daemon 内部健康快照领域模型。
//!
//! 健康快照面向值守与容量审计，只暴露内部事实源聚合指标，
//! 不要求调用方提供 Session ID，也不暴露 Session 管理能力。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 单个事实源按状态聚合的计数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthStatusCounts {
    /// 记录总数。
    pub total: u64,
    /// 各状态计数；键按状态名排序，便于稳定对比。
    pub by_status: BTreeMap<String, u64>,
}

impl HealthStatusCounts {
    /// 由状态计数流构造，并同时汇总总数。
    #[must_use]
    pub fn from_statuses<I, S>(statuses: I) -> Self
    where
        I: IntoIterator<Item = (S, u64)>,
        S: Into<String>,
    {
        let mut by_status: BTreeMap<String, u64> = BTreeMap::new();
        let mut total = 0_u64;
        for (status, count) in statuses {
            total = total.saturating_add(count);
            let entry = by_status.entry(status.into()).or_default();
            *entry = entry.saturating_add(count);
        }
        Self { total, by_status }
    }
}

/// 积压等待指标。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthBacklog {
    /// 仍在等待处理的数量。
    pub pending: u64,
    /// 最老积压记录距今的毫秒数；无积压时为空。
    pub oldest_age_ms: Option<u64>,
}

/// Daemon Host 自身的租约状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthHostState {
    /// Host 实例 ID。
    pub host_id: String,
    /// 当前 generation。
    pub generation: String,
    /// Host 生命周期状态。
    pub status: String,
    /// Host lease 绝对到期时间。
    pub lease_expires_at_ms: u64,
    /// Host lease 剩余毫秒数。
    pub lease_remaining_ms: u64,
}

/// 渠道投递（outbox）健康指标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryHealth {
    /// 投递记录按状态计数。
    pub counts: HealthStatusCounts,
    /// 未完成投递的积压指标。
    pub backlog: HealthBacklog,
}

/// 不依赖 Session ID 的 Daemon 内部健康快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonHealthSnapshot {
    /// 快照生成时间。
    pub generated_at_ms: u64,
    /// 当前 Host 租约状态。
    pub host: HealthHostState,
    /// Interaction 按状态计数。
    pub interactions: HealthStatusCounts,
    /// Goal 按状态计数。
    pub goals: HealthStatusCounts,
    /// Execution 按状态计数。
    pub executions: HealthStatusCounts,
    /// Execution 积压指标。
    pub execution_backlog: HealthBacklog,
    /// 渠道投递指标；宿主未接入投递存储时为空。
    pub deliveries: Option<DeliveryHealth>,
    /// 当前实时事件流订阅数。
    ///
    /// 一条连接可以并存多路订阅（车道流与子代理会话流各占一路），因此这个数
    /// 就是「daemon 侧此刻在为几路订阅投递事件」，也是订阅释放是否真的发生的
    /// 唯一可观测判据。
    #[serde(default)]
    pub stream_subscribers: u64,
}

#[cfg(test)]
mod tests {
    use super::HealthStatusCounts;

    #[test]
    fn status_counts_fold_duplicates_and_total() {
        let counts =
            HealthStatusCounts::from_statuses([("queued", 2_u64), ("running", 1), ("queued", 3)]);
        assert_eq!(counts.total, 6);
        assert_eq!(counts.by_status.get("queued"), Some(&5));
        assert_eq!(counts.by_status.get("running"), Some(&1));
    }
}
