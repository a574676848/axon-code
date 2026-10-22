//! Daemon 与系统环境的端口。

pub mod ingress_cursor_store;
pub mod ipc_server;
pub mod service_manager;
pub mod transport_adapter;

/// 渠道投递健康查询端口。
pub mod delivery_health_store;

pub use delivery_health_store::DeliveryHealthStore;
pub use ingress_cursor_store::{IngressCursor, IngressCursorStore};
pub use ipc_server::IpcServer;
pub use service_manager::ServiceManager;
pub use transport_adapter::TransportAdapter;

/// 不依赖操作系统实现的时间源。
pub trait DaemonClock: Send + Sync {
    /// 当前 Unix 时间毫秒。
    fn now_ms(&self) -> u64;
}

/// 宿主进程身份端口。
pub trait ProcessIdentity: Send + Sync {
    /// 操作系统 PID。
    fn pid(&self) -> u32;
    /// 用于防 PID 重用的进程启动身份。
    fn process_start_id(&self) -> Option<String>;
}
