//! Transport ingress cursor 的持久化端口。
/// 独立于 Session event cursor 的 ingress 水位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressCursor {
    /// 适配器名称。
    pub adapter: String,
    /// 适配器分区。
    pub partition: String,
    /// 外部事件 ID。
    pub event_id: String,
    /// 分区序号。
    pub sequence: u64,
}
/// 以 adapter + partition 为键保存 ingress cursor。
pub trait IngressCursorStore: Send + Sync {
    /// 读取当前水位。
    fn load(&self, adapter: &str, partition: &str) -> Result<Option<IngressCursor>, String>;
    /// 在 Action durable 入账后推进水位。
    fn save(&self, cursor: &IngressCursor) -> Result<(), String>;
}
