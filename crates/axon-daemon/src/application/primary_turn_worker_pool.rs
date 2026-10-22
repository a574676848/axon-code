//! JARVIS 主 turn lane 的长期存活 worker pool。

use std::{
    fmt,
    sync::{
        Arc, Mutex,
        mpsc::{self, TrySendError},
    },
    thread::{self, JoinHandle},
};

const WORKER_THREAD_NAME_PREFIX: &str = "axon-primary-turn";
const WORKER_SPAWN_ERROR_PREFIX: &str = "创建 Primary turn worker 失败";
const WORKER_QUEUE_CLOSED_ERROR: &str = "Primary turn worker pool 已关闭";

type LaneJob = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone)]
pub(super) struct PrimaryTurnWorkerPool {
    inner: Arc<PrimaryTurnWorkerPoolInner>,
}

#[derive(Clone, Default)]
pub(super) struct PrimaryTurnWorkerPoolSlot {
    pool: Arc<Mutex<Option<PrimaryTurnWorkerPool>>>,
}

struct PrimaryTurnWorkerPoolInner {
    worker_count: usize,
    sender: Option<mpsc::SyncSender<LaneJob>>,
    workers: Vec<JoinHandle<()>>,
}

impl PrimaryTurnWorkerPool {
    pub(super) fn new_with_capacity(
        worker_count: usize,
        queue_capacity: usize,
    ) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = spawn_workers(worker_count, receiver)?;
        Ok(Self {
            inner: Arc::new(PrimaryTurnWorkerPoolInner {
                worker_count,
                sender: Some(sender),
                workers,
            }),
        })
    }

    pub(super) fn worker_count(&self) -> usize {
        self.inner.worker_count
    }

    pub(super) fn execute<F>(&self, job: F) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        self.inner
            .sender
            .as_ref()
            .ok_or_else(|| WORKER_QUEUE_CLOSED_ERROR.to_owned())?
            .send(Box::new(job))
            .map_err(|_| WORKER_QUEUE_CLOSED_ERROR.to_owned())
    }

    pub(super) fn try_execute<F>(&self, job: F) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        let sender = self
            .inner
            .sender
            .as_ref()
            .ok_or_else(|| WORKER_QUEUE_CLOSED_ERROR.to_owned())?;
        match sender.try_send(Box::new(job)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("Primary turn worker pool 队列已满".to_owned()),
            Err(TrySendError::Disconnected(_)) => Err(WORKER_QUEUE_CLOSED_ERROR.to_owned()),
        }
    }
}

impl PrimaryTurnWorkerPoolSlot {
    pub(super) fn get_or_create(
        &self,
        worker_count: usize,
    ) -> Result<PrimaryTurnWorkerPool, String> {
        self.get_or_create_with_capacity(worker_count, worker_count)
    }

    pub(super) fn get_or_create_with_capacity(
        &self,
        worker_count: usize,
        queue_capacity: usize,
    ) -> Result<PrimaryTurnWorkerPool, String> {
        let mut pool = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(current) = pool
            .as_ref()
            .filter(|current| current.worker_count() == worker_count)
        {
            return Ok(current.clone());
        }
        let replacement = PrimaryTurnWorkerPool::new_with_capacity(worker_count, queue_capacity)?;
        *pool = Some(replacement.clone());
        Ok(replacement)
    }
}

impl fmt::Debug for PrimaryTurnWorkerPool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrimaryTurnWorkerPool")
            .field("worker_count", &self.worker_count())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PrimaryTurnWorkerPoolSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let worker_count = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(PrimaryTurnWorkerPool::worker_count);
        formatter
            .debug_struct("PrimaryTurnWorkerPoolSlot")
            .field("worker_count", &worker_count)
            .finish()
    }
}

impl Drop for PrimaryTurnWorkerPoolInner {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            drop(worker.join());
        }
    }
}

fn spawn_workers(
    worker_count: usize,
    receiver: Arc<Mutex<mpsc::Receiver<LaneJob>>>,
) -> Result<Vec<JoinHandle<()>>, String> {
    let mut workers = Vec::with_capacity(worker_count);
    for worker_index in 0..worker_count {
        let receiver = Arc::clone(&receiver);
        let thread_name = format!("{WORKER_THREAD_NAME_PREFIX}-{worker_index}");
        let worker = thread::Builder::new()
            .name(thread_name)
            .spawn(move || run_worker(receiver))
            .map_err(|error| format!("{WORKER_SPAWN_ERROR_PREFIX}：{error}"))?;
        workers.push(worker);
    }
    Ok(workers)
}

fn run_worker(receiver: Arc<Mutex<mpsc::Receiver<LaneJob>>>) {
    loop {
        let job = receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recv();
        let Ok(job) = job else {
            break;
        };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn try_execute_rejects_when_worker_and_queue_are_full() {
        let pool = PrimaryTurnWorkerPool::new_with_capacity(1, 1).expect("创建有界池");
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_started = Arc::clone(&started);
        let worker_release = Arc::clone(&release);
        pool.execute(move || {
            worker_started.wait();
            worker_release.wait();
        })
        .expect("提交首个任务");
        started.wait();

        pool.try_execute(|| {}).expect("队列应容纳一个等待任务");
        assert!(pool.try_execute(|| {}).is_err());

        release.wait();
    }
}
