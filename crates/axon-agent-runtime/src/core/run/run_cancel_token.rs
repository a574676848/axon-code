use std::sync::{
    Arc, Condvar, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use tokio::sync::Notify;

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    changed: Condvar,
    wait_lock: Mutex<()>,
    notify: Notify,
    children: Mutex<Vec<Weak<CancellationState>>>,
}

impl CancellationState {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            changed: Condvar::new(),
            wait_lock: Mutex::new(()),
            notify: Notify::new(),
            children: Mutex::new(Vec::new()),
        }
    }

    fn cancel(&self) {
        if self.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }
        self.changed.notify_all();
        self.notify.notify_waiters();
        let children = {
            let mut children = self
                .children
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            children.retain(|child| child.strong_count() > 0);
            children
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        };
        for child in children {
            child.cancel();
        }
    }

    fn register_child(&self, child: &Arc<Self>) {
        let mut children = self
            .children
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.cancelled.load(Ordering::SeqCst) {
            drop(children);
            child.cancel();
            return;
        }
        children.retain(|candidate| candidate.strong_count() > 0);
        children.push(Arc::downgrade(child));
    }
}

/// 跨运行时边界传递的协作式取消令牌。
///
/// 令牌可被同步检查，也可在异步 I/O、限流和退避等待中直接等待取消通知。
#[derive(Debug, Clone)]
pub struct RunCancelToken {
    state: Arc<CancellationState>,
}

impl Default for RunCancelToken {
    fn default() -> Self {
        Self {
            state: Arc::new(CancellationState::new()),
        }
    }
}

impl RunCancelToken {
    /// 创建继承父级取消状态的子令牌。
    #[must_use]
    pub fn child_token(&self) -> Self {
        let child = Self::default();
        self.state.register_child(&child.state);
        child
    }

    /// 触发取消并唤醒所有等待者。
    pub fn cancel(&self) {
        self.state.cancel();
    }

    /// 返回当前令牌是否已取消。
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    /// 阻塞等待本令牌取消，或在等待期到达时返回。
    #[must_use]
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        if self.is_cancelled() {
            return true;
        }
        let lock = self
            .state
            .wait_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = self
            .state
            .changed
            .wait_timeout_while(lock, timeout, |()| !self.is_cancelled())
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.is_cancelled()
    }

    /// 异步等待本令牌取消。
    pub async fn cancelled(&self) {
        let notified = self.state.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }

    /// 判断两个令牌是否代表同一次执行。
    #[must_use]
    pub fn same_execution(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    /// 构造一个已取消的测试令牌。
    #[cfg(test)]
    #[must_use]
    pub fn cancelled_for_test() -> Self {
        let token = Self::default();
        token.cancel();
        token
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::RunCancelToken;

    const CANCEL_TRIGGER_DELAY: Duration = Duration::from_millis(5);
    const CANCEL_WAIT_TIMEOUT: Duration = Duration::from_millis(100);

    #[test]
    fn parent_cancellation_wakes_child_sync_waiter() {
        let parent = RunCancelToken::default();
        let child = parent.child_token();
        let trigger = parent.clone();
        let waiter = std::thread::spawn(move || child.wait_timeout(CANCEL_WAIT_TIMEOUT));

        std::thread::sleep(CANCEL_TRIGGER_DELAY);
        trigger.cancel();

        assert!(waiter.join().unwrap());
    }

    #[test]
    fn parent_cancellation_wakes_child_async_waiter() {
        let parent = RunCancelToken::default();
        let child = parent.child_token();
        let trigger = parent.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();

        let waiter = runtime.block_on(async move {
            let cancellation = child.cancelled();
            std::thread::spawn(move || {
                std::thread::sleep(CANCEL_TRIGGER_DELAY);
                trigger.cancel();
            });
            tokio::time::timeout(CANCEL_WAIT_TIMEOUT, cancellation).await
        });

        assert!(waiter.is_ok());
    }
}
