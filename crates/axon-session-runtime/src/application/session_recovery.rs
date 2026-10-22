//! Session recovery 应用服务。

use axon_domain::DaemonSessionId;
use axon_session::{DaemonHostRecord, RuntimeStore, RuntimeStoreError};

/// 进程重启后恢复未完成 Session 的协调器。
pub struct SessionRecovery<S> {
    store: S,
}

impl<S: RuntimeStore> SessionRecovery<S> {
    /// 创建恢复协调器。
    #[must_use]
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// 限量接管 lease 已失效的非终态 Session。
    pub fn recover_batch(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<DaemonSessionId>, RuntimeStoreError> {
        self.store
            .recover_expired_sessions(host, now_ms, limit)
            .map(|records| {
                records
                    .into_iter()
                    .map(|record| record.session_id)
                    .collect()
            })
    }
}
