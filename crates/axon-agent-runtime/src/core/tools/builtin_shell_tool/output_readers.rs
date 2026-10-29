//! 子进程输出读取线程的句柄集合与终态收口。
//!
//! 子进程退出信号（`try_wait` 返回 `Some`）**早于**输出线程读到 EOF：管道里可能还有
//! 未取走的字节，读取线程也还没把它们写进共享缓冲。若在此刻直接组装终态回执，就会
//! 拿到空缓冲，实测表现为 `status=Completed`、`exit_code=0`、`original_chars=0`
//! 的「零输出回执」。因此终态必须先把读取线程排干（`drain`），再取回执。

use std::{
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// 收口时的轮询粒度：既不让出过多 CPU，也保证排干后立刻返回。
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// 全部输出读取线程的句柄。
///
/// stdout / stderr 各一条线程（PTY 只有一条），二者都必须排干：任一未收口都会让
/// 同名流在终态回执里凭空少掉一段。
#[derive(Default)]
pub(crate) struct OutputReaders {
    handles: Vec<JoinHandle<()>>,
}

impl OutputReaders {
    pub(crate) fn push(&mut self, handle: JoinHandle<()>) {
        self.handles.push(handle);
    }

    /// 等所有读取线程读到 EOF 并写完共享缓冲，最多等到 `timeout`。
    ///
    /// 超时后仍未结束的线程直接分离（`JoinHandle` 丢弃即分离）：它仍会继续把后续输出
    /// 写进共享缓冲，只是本次收口不再等它。没有超时就会把「后台守护进程继承了管道、
    /// 写端永不关闭」的一次性命令拖成无限等待。
    pub(crate) fn drain(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.handles.iter().any(|handle| !handle.is_finished()) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            thread::sleep(DRAIN_POLL_INTERVAL.min(remaining));
        }
        for handle in std::mem::take(&mut self.handles) {
            // 只回收已结束的线程：`join` 未结束的线程会永久阻塞，与超时语义冲突。
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

/// 测试钩子：把输出读取线程停在「已读到字节、尚未写进共享缓冲」处。
///
/// 生产构建中不存在。测试用它把「子进程已退出而输出线程尚未收口」的竞态变成
/// 确定时序，从而稳定复现终态回执丢输出的缺陷
/// （见 `process_manager::output_drain_tests`）。
#[cfg(test)]
pub(crate) mod test_hook {
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread::{self, ThreadId},
        time::Duration,
    };

    static RELEASED: AtomicBool = AtomicBool::new(true);
    static HOLD_THREAD: Mutex<Option<ThreadId>> = Mutex::new(None);

    /// 让**当前线程**随后启动的输出线程在第一次读到字节时停住，直到 `release`。
    pub(crate) fn hold_next_reader_of_current_thread() {
        *HOLD_THREAD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(thread::current().id());
        RELEASED.store(false, Ordering::SeqCst);
    }

    pub(crate) fn release() {
        RELEASED.store(true, Ordering::SeqCst);
    }

    /// 必须由启动输出线程的线程调用：判定依据是线程标识，只影响本次 spawn 的线程，
    /// 不会拖住并行运行的其他用例的输出线程。
    pub(crate) fn holds_current_thread() -> bool {
        if RELEASED.load(Ordering::SeqCst) {
            return false;
        }
        *HOLD_THREAD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            == Some(thread::current().id())
    }

    /// 在输出线程内调用；`hold` 是 `holds_current_thread` 在 spawn 时的判定结果。
    pub(crate) fn wait_until_released(hold: bool) {
        while hold && !RELEASED.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, sync::Mutex as StdMutex};

    #[test]
    fn drain_waits_until_every_reader_has_finished_writing() {
        let written = Arc::new(StdMutex::new(Vec::new()));
        let mut readers = OutputReaders::default();
        for (index, delay_ms) in [(0_u8, 0_u64), (1, 40)] {
            let written = Arc::clone(&written);
            readers.push(thread::spawn(move || {
                thread::sleep(Duration::from_millis(delay_ms));
                written
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(index);
            }));
        }

        readers.drain(Duration::from_secs(5));

        assert_eq!(
            *written
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![0, 1],
            "drain 必须等到每个读取线程真正写完，否则终态回执会漏掉最后一段输出"
        );
    }

    #[test]
    fn drain_gives_up_once_the_timeout_expires() {
        let mut readers = OutputReaders::default();
        readers.push(thread::spawn(|| thread::sleep(Duration::from_secs(30))));

        let started = Instant::now();
        readers.drain(Duration::from_millis(20));

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "读取线程不结束时 drain 必须按超时放弃，不得永久阻塞"
        );
    }
}
