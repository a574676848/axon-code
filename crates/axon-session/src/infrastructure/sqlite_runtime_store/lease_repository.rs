//! Host/Session lease repository 边界。
use crate::domain::DaemonHostRecord;
use crate::ports::{RuntimeStore, RuntimeStoreError};
/// lease repository 视图。
#[derive(Debug, Clone, Copy)]
pub struct LeaseRepository<'a, S> {
    /// 底层运行时事实源。
    pub store: &'a S,
}
impl<'a, S: RuntimeStore> LeaseRepository<'a, S> {
    /// 创建 repository 视图。
    #[must_use]
    pub fn new(store: &'a S) -> Self {
        Self { store }
    }
    /// 续租 Host。
    pub fn renew(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        duration_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        self.store.renew_host(host, now_ms, duration_ms)
    }
}
