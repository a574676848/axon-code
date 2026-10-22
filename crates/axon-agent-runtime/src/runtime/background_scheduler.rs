//! 本机后台调度线程公共骨架。
//!
//! 该模块只负责线程生命周期、固定间隔 tick 与结果回传，不承载业务语义。

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// 后台调度线程句柄。
pub struct BackgroundScheduler<T> {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    receiver: Receiver<T>,
}

impl<T: Send + 'static> BackgroundScheduler<T> {
    /// 启动固定间隔后台调度线程。
    #[must_use]
    pub fn start<F>(interval_ms: u64, mut tick: F) -> Self
    where
        F: FnMut() -> Option<T> + Send + 'static,
    {
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let (sender, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            while worker_running.load(Ordering::SeqCst) {
                if let Some(item) = tick() {
                    let _ = sender.send(item);
                }
                thread::sleep(Duration::from_millis(interval_ms.max(1)));
            }
        });
        Self {
            running,
            thread: Some(thread),
            receiver,
        }
    }

    /// 等待下一条调度结果。
    ///
    /// # Errors
    ///
    /// 当超时或后台线程退出导致 channel 关闭时返回接收错误。
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    /// 停止后台调度线程。
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl<T> Drop for BackgroundScheduler<T> {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
