//! Daemon 投递健康探测的 CLI 适配。
//!
//! Daemon 只依赖端口，投递存储的具体位置由组合根解析后注入。

use axon_daemon::ports::DeliveryHealthStore;
use axon_domain::DeliveryHealth;
use axon_remote_runtime::DeliveryStore;

/// 基于渠道 outbox 的健康探测。
#[derive(Debug, Clone)]
pub struct CliDeliveryHealthStore {
    store: DeliveryStore,
}

impl CliDeliveryHealthStore {
    /// 绑定一个 durable 投递存储。
    #[must_use]
    pub fn new(store: DeliveryStore) -> Self {
        Self { store }
    }
}

impl DeliveryHealthStore for CliDeliveryHealthStore {
    fn read_delivery_health(&self, now_ms: u64) -> Result<DeliveryHealth, String> {
        self.store
            .summary(now_ms)
            .map_err(|error| format!("读取投递健康指标失败：{error:?}"))
    }
}
