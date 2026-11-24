//! Runtime 内部健康指标查询端口。
//!
//! 健康指标只做有界聚合，不读取明细，也不要求调用方提供 Session ID。

use axon_domain::{HealthBacklog, HealthStatusCounts};

/// Runtime Store 一次健康聚合的全部结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeHealthCounters {
    /// Interaction 按状态计数。
    pub interactions: HealthStatusCounts,
    /// Goal 按状态计数。
    pub goals: HealthStatusCounts,
    /// Execution 按状态计数。
    pub executions: HealthStatusCounts,
    /// Execution 积压指标。
    pub execution_backlog: HealthBacklog,
}

/// 健康聚合端口；由宿主基础设施实现。
pub trait HealthStore {
    /// 读取当前事实源的聚合指标。
    fn read_runtime_health(
        &self,
        now_ms: u64,
    ) -> Result<RuntimeHealthCounters, super::RuntimeStoreError>;
}
