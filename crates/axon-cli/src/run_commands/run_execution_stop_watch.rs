//! 后台执行的终止看门狗。
//!
//! 取消或暂停目标时，调度侧只在 `work_items` 上写一条「请求取消」标记；运行侧必须自己
//! 消费它，否则界面显示已停止、provider 仍然继续跑。这里是唯一的消费点：执行期间按有界
//! 周期查一次存在性，命中即触发本轮的取消令牌，由 provider 循环负责收口。
//!
//! 周期固定 2 秒，只读 `(session_id, status)` 索引前缀，不写库、不改执行结果。

use std::{
    path::Path,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use axon_domain::DaemonSessionId;
use axon_session::RuntimeStore as _;

/// 取消标记的轮询周期。
const STOP_WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// 后台执行的终止看门狗；Drop 即停止并等待线程退出。
pub(super) struct ExecutionStopWatch {
    stop: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl ExecutionStopWatch {
    /// 起一条看门狗：命中取消标记即触发 `token`。
    ///
    /// 取不到库或会话标识非法时只跳过本轮检查，不中止执行：终止是尽力而为的收口，
    /// 不能因为看门狗自己出问题把一次正常执行打断。
    pub(super) fn start(
        config_path: &Path,
        session_id: &str,
        token: super::RunCancelToken,
    ) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_stop = Arc::clone(&stop);
        let config_path = config_path.to_path_buf();
        let session_id = session_id.to_owned();
        let handle = thread::Builder::new()
            .name("execution-stop-watch".to_owned())
            .spawn(move || {
                let (lock, cvar) = &*thread_stop;
                loop {
                    let guard = lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let (guard, _) = cvar
                        .wait_timeout(guard, STOP_WATCH_INTERVAL)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let stopping = *guard;
                    drop(guard);
                    if stopping {
                        break;
                    }
                    let Ok(session) = DaemonSessionId::new(session_id.clone()) else {
                        break;
                    };
                    let Ok(store) = super::run_turn_metrics::shared_store(&config_path) else {
                        continue;
                    };
                    if store
                        .session_cancellation_requested(&session)
                        .unwrap_or(false)
                    {
                        token.cancel();
                        break;
                    }
                }
            })
            .ok();
        Self { stop, handle }
    }
}

impl Drop for ExecutionStopWatch {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.stop;
        if let Ok(mut guard) = lock.lock() {
            *guard = true;
            cvar.notify_all();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
