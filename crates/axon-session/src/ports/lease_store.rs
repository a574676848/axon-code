//! Lease 持久化端口视图。
use super::runtime_store::RuntimeStore;
/// Lease Store 端口；实现由统一 RuntimeStore 提供。
pub trait LeaseStore: RuntimeStore {}
impl<T: RuntimeStore> LeaseStore for T {}
