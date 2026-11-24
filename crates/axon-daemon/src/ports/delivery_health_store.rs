//! 渠道投递（outbox）健康查询端口。
//!
//! 投递存储由渠道适配层持有，因此由组合根注入实现；未接入时健康快照不包含投递指标。

use axon_domain::DeliveryHealth;

/// 投递积压聚合端口。
pub trait DeliveryHealthStore: Send + Sync {
    /// 读取投递积压聚合指标。
    fn read_delivery_health(&self, now_ms: u64) -> Result<DeliveryHealth, String>;
}
