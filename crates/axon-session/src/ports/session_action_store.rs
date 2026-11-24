//! Action 持久化端口视图。
use super::runtime_store::RuntimeStore;
/// Action Store 端口；实现由统一 RuntimeStore 提供。
pub trait SessionActionStore: RuntimeStore {}
impl<T: RuntimeStore> SessionActionStore for T {}
