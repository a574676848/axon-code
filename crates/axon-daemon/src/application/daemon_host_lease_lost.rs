//! 主 turn 租约丢失时的收口接线。
//!
//! 单独成篇：宿主本体已经接近行数硬限，而这里只有一个泛型实现方法。

use super::super::primary_turn_service::LeaseLostHook;
use super::DaemonHost;
use crate::ports::ProcessIdentity;
use axon_session::RuntimeStore;
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 接上主 turn 租约丢失时的收口钩子。
    ///
    /// 组合根在这里接运行侧的中断入口：与用户按 ESC 走同一条取消令牌，因此租约一旦被
    /// 证明不在自己手上，这一轮会在下一个检查点收口，而不是继续空跑。
    pub fn with_lease_lost(mut self, hook: LeaseLostHook) -> Self {
        self.lease_lost = Some(hook);
        self
    }
}
