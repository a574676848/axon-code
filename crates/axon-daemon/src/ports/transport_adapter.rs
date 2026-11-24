//! Transport Adapter 注册端口。

/// 入口适配器的最小生命周期契约。
pub trait TransportAdapter: Send + Sync {
    /// 稳定适配器名称。
    fn name(&self) -> &'static str;
    /// 启动入站监听。
    fn start(&self) -> Result<(), String>;
    /// 停止入站监听。
    fn stop(&self) -> Result<(), String>;
}
