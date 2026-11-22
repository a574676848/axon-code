//! Gateway 的有界连接准入：许可计数与它的活动连接读数。
//!
//! 从 [`super`] 整块搬出：内核本身已接近单文件行数硬限，而连接准入与适配器编排、
//! 生命周期属于两件事。搬出只改位置，不改任何语义：满载立即失败、许可析构即归还、
//! 默认上限与 IPC 同级。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{DEFAULT_CONNECTION_LIMIT, GatewayKernelError};

/// 有界 Gateway 连接准入器。
#[derive(Debug)]
pub struct GatewayConnectionLimiter {
    limit: usize,
    active: Arc<AtomicUsize>,
}

impl GatewayConnectionLimiter {
    /// 创建连接准入器；零值使用默认 256 上限。
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 使用 IPC 同级的默认连接上限。
    #[must_use]
    pub fn default_limit() -> Self {
        Self::new(DEFAULT_CONNECTION_LIMIT)
    }

    /// 尝试取得连接许可，满载时立即失败，不阻塞调用线程。
    pub fn try_acquire(&self) -> Result<GatewayConnectionPermit, GatewayKernelError> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < self.limit).then_some(current.saturating_add(1))
            })
            .map(|_| GatewayConnectionPermit {
                active: Arc::clone(&self.active),
            })
            .map_err(|_| GatewayKernelError::ConnectionLimitReached(self.limit))
    }

    /// 当前活动连接数。
    #[must_use]
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

/// 一个活动连接许可。
pub struct GatewayConnectionPermit {
    active: Arc<AtomicUsize>,
}

impl Drop for GatewayConnectionPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl std::fmt::Debug for GatewayConnectionPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("GatewayConnectionPermit").finish()
    }
}
