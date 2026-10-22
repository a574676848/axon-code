//! Event 持久化端口视图。
use super::runtime_store::RuntimeStore;
/// Event Store 端口；实现由统一 RuntimeStore 提供。
pub trait SessionEventStore: RuntimeStore {}
impl<T: RuntimeStore> SessionEventStore for T {}
