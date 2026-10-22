//! Schedule 持久化端口视图。
use super::runtime_store::RuntimeStore;
/// Schedule Store 端口；实现由统一 RuntimeStore 提供。
pub trait ScheduleStore: RuntimeStore {}
impl<T: RuntimeStore> ScheduleStore for T {}
