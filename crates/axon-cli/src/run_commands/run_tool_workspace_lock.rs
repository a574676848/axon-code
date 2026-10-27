//! 有界写优先的工作区读写闸（PWC-1 `S-7` 规则 7）。
//!
//! 规则：
//! 1. 写者在排队时，新读者默认等待（写优先）；
//! 2. 写者等待超过饥饿窗口后取得优先权：新读者不再进入，等当前读者收尾即可；
//! 3. 整工作区写释放后，已排队的路径请求优先于下一个写者；
//! 4. 读者队列清空后才撤销读者优先级，避免"只放行一个读者"。
//!
//! 两条边界同时成立：写者最多等当前读者的剩余工作；读者最多等一个写者的
//! 执行时间（写者优先权生效后不会再被新写者插队）。

use std::{
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

/// 与 `limits.yaml` 的 `execution.unit_timeout_ms`（600000ms）同口径：
/// 写者等待超过该窗口后取得优先权，L1 请求不再被无限压住。
pub(super) const DEFAULT_STARVATION_WINDOW: Duration = Duration::from_millis(600_000);

#[derive(Default)]
struct GateState {
    readers: usize,
    writer: bool,
    queued_readers: usize,
    queued_writers: usize,
    prefer_readers: bool,
    writers_priority: bool,
}

pub(super) struct FairWorkspaceLock {
    state: Mutex<GateState>,
    ready: Condvar,
    starvation_window: Duration,
}

impl Default for FairWorkspaceLock {
    fn default() -> Self {
        Self::with_starvation_window(DEFAULT_STARVATION_WINDOW)
    }
}

impl FairWorkspaceLock {
    pub(super) fn with_starvation_window(starvation_window: Duration) -> Self {
        Self {
            state: Mutex::new(GateState::default()),
            ready: Condvar::new(),
            starvation_window,
        }
    }

    pub(super) fn read(&self) -> WorkspaceReadGuard<'_> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.queued_readers += 1;
        loop {
            let admitted = !state.writer
                && !state.writers_priority
                && (state.queued_writers == 0 || state.prefer_readers);
            if admitted {
                state.queued_readers -= 1;
                state.readers += 1;
                if state.queued_readers == 0 {
                    // 队列清空才撤销读者优先，避免"只放行一个读者"。
                    state.prefer_readers = false;
                }
                return WorkspaceReadGuard { lock: self };
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub(super) fn write(&self) -> WorkspaceWriteGuard<'_> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.queued_writers += 1;
        let queued_at = Instant::now();
        loop {
            let waited = queued_at.elapsed();
            // 等待超过饥饿窗口：关掉新读者入口，并撤销读者优先，避免两个标志互锁。
            if waited >= self.starvation_window && !state.writers_priority {
                state.writers_priority = true;
                state.prefer_readers = false;
                self.ready.notify_all();
            }
            let readers_priority = state.prefer_readers && state.queued_readers > 0;
            if !state.writer && state.readers == 0 && !readers_priority {
                state.queued_writers -= 1;
                state.writer = true;
                state.writers_priority = false;
                return WorkspaceWriteGuard { lock: self };
            }
            // 已过窗口时以短超时轮询，避免 0 超时忙等。
            let timeout = if waited >= self.starvation_window {
                Duration::from_millis(50)
            } else {
                self.starvation_window - waited
            };
            state = self
                .ready
                .wait_timeout(state, timeout)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    fn release_read(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.readers = state.readers.saturating_sub(1);
        drop(state);
        self.ready.notify_all();
    }

    fn release_write(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.writer = false;
        // 已排队的路径请求优先于下一个整工作区写；写者优先期内不抢回优先权。
        state.prefer_readers = state.queued_readers > 0 && !state.writers_priority;
        drop(state);
        self.ready.notify_all();
    }
}

pub(super) struct WorkspaceReadGuard<'a> {
    lock: &'a FairWorkspaceLock,
}

pub(super) struct WorkspaceWriteGuard<'a> {
    lock: &'a FairWorkspaceLock,
}

impl Drop for WorkspaceReadGuard<'_> {
    fn drop(&mut self) {
        self.lock.release_read();
    }
}

impl Drop for WorkspaceWriteGuard<'_> {
    fn drop(&mut self) {
        self.lock.release_write();
    }
}

impl std::fmt::Debug for FairWorkspaceLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        formatter
            .debug_struct("FairWorkspaceLock")
            .field("readers", &state.readers)
            .field("writer", &state.writer)
            .field("queued_readers", &state.queued_readers)
            .field("queued_writers", &state.queued_writers)
            .field("writers_priority", &state.writers_priority)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use super::*;

    #[test]
    fn queued_writer_blocks_new_readers() {
        let gate = Arc::new(FairWorkspaceLock::with_starvation_window(
            Duration::from_secs(30),
        ));
        let held = gate.read();
        let writer_gate = Arc::clone(&gate);
        let writer = thread::spawn(move || {
            let _guard = writer_gate.write();
        });
        thread::sleep(Duration::from_millis(50));
        let reader_started = Arc::new(AtomicUsize::new(0));
        let reader_gate = Arc::clone(&gate);
        let flag = Arc::clone(&reader_started);
        let reader = thread::spawn(move || {
            let _guard = reader_gate.read();
            flag.store(1, Ordering::SeqCst);
        });
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            reader_started.load(Ordering::SeqCst),
            0,
            "写者排队时新读者必须等待"
        );
        drop(held);
        writer.join().unwrap();
        reader.join().unwrap();
        assert_eq!(reader_started.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn writer_gets_through_after_the_starvation_window() {
        let gate = Arc::new(FairWorkspaceLock::with_starvation_window(
            Duration::from_millis(60),
        ));
        let held = gate.read();
        let writer_gate = Arc::clone(&gate);
        let (writer_tx, writer_rx) = mpsc::channel();
        let writer = thread::spawn(move || {
            let _guard = writer_gate.write();
            let _ = writer_tx.send(());
        });
        // 持续到达的读者不得把写者饿死。
        let mut readers = Vec::new();
        for _ in 0..16 {
            let reader_gate = Arc::clone(&gate);
            readers.push(thread::spawn(move || {
                let _guard = reader_gate.read();
                thread::sleep(Duration::from_millis(5));
            }));
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(80));
        drop(held);
        writer_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("写者必须在饥饿窗口后取得锁");
        writer.join().unwrap();
        for reader in readers {
            let _ = reader.join();
        }
    }

    #[test]
    fn writer_priority_and_reader_priority_never_interlock() {
        let gate = Arc::new(FairWorkspaceLock::with_starvation_window(
            Duration::from_millis(50),
        ));
        let held = gate.read();
        let writer_gate = Arc::clone(&gate);
        let (writer_tx, writer_rx) = mpsc::channel();
        let writer = thread::spawn(move || {
            let _guard = writer_gate.write();
            let _ = writer_tx.send(());
        });
        // 写者越过窗口取得优先权后，再排一个读者。
        thread::sleep(Duration::from_millis(80));
        let reader_gate = Arc::clone(&gate);
        let (reader_tx, reader_rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let _guard = reader_gate.read();
            let _ = reader_tx.send(());
        });
        thread::sleep(Duration::from_millis(30));
        drop(held);
        writer_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("写者必须先于排队读者完成");
        reader_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("读者不得被写者优先永久挡住");
        writer.join().unwrap();
        reader.join().unwrap();
    }
    #[test]
    fn queued_readers_go_before_the_next_writer_after_release() {
        let gate = Arc::new(FairWorkspaceLock::with_starvation_window(
            Duration::from_secs(30),
        ));
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let held = gate.write();

        let mut readers = Vec::new();
        for label in ["reader-1", "reader-2"] {
            let reader_gate = Arc::clone(&gate);
            let reader_order = Arc::clone(&order);
            readers.push(thread::spawn(move || {
                let guard = reader_gate.read();
                reader_order
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(label);
                drop(guard);
            }));
        }
        thread::sleep(Duration::from_millis(20));

        let writer_gate = Arc::clone(&gate);
        let writer_order = Arc::clone(&order);
        let writer = thread::spawn(move || {
            let guard = writer_gate.write();
            writer_order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("writer");
            drop(guard);
        });
        thread::sleep(Duration::from_millis(20));

        drop(held);
        for reader in readers {
            reader.join().unwrap();
        }
        writer.join().unwrap();
        let order = order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let writer_position = order
            .iter()
            .position(|label| *label == "writer")
            .expect("写者必须执行");
        assert_eq!(
            writer_position, 2,
            "释放后应先放行两个排队读者，再轮到写者，实际顺序 {order:?}"
        );
    }
}
