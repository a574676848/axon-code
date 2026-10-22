use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, OnceLock},
    thread,
};

use super::RemoteCliContext;

pub(super) const ASYNC_PROCESSING_QUEUE_DEPTH: usize = 32;
pub(super) const ASYNC_PROCESSING_WORKER_COUNT: usize = 4;

#[derive(Clone)]
struct ProcessingStartTask {
    context: RemoteCliContext,
    message_id: String,
}

struct ProcessingStartQueue {
    capacity: usize,
    state: Mutex<VecDeque<ProcessingStartTask>>,
    wake: Condvar,
}

impl ProcessingStartQueue {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        }
    }

    fn submit(&self, task: ProcessingStartTask) {
        let Ok(mut state) = self.state.lock() else {
            axon_logger::warning("Feishu processing LRU 队列锁不可用，已丢弃 reaction 请求");
            return;
        };
        if let Some(index) = state
            .iter()
            .position(|queued| queued.message_id == task.message_id)
        {
            state.remove(index);
        }
        if state.len() >= self.capacity
            && let Some(evicted) = state.pop_front()
        {
            axon_logger::warning(&format!(
                "Feishu processing LRU 队列已满，淘汰旧 reaction 请求 message_id={}",
                evicted.message_id
            ));
        }
        state.push_back(task);
        self.wake.notify_one();
    }

    fn next(&self) -> Option<ProcessingStartTask> {
        let mut state = self.state.lock().ok()?;
        loop {
            if let Some(task) = state.pop_front() {
                return Some(task);
            }
            state = self.wake.wait(state).ok()?;
        }
    }
}

static PROCESSING_START_QUEUE: OnceLock<Arc<ProcessingStartQueue>> = OnceLock::new();

pub(super) fn start_processing_async<F>(
    context: &RemoteCliContext,
    message_id: &str,
    start_processing_message: F,
) where
    F: Fn(&RemoteCliContext, &str) + Send + Sync + 'static,
{
    if !context.feishu_websocket.enabled || !context.feishu_websocket.enable_notification_sending {
        return;
    }
    let queue = PROCESSING_START_QUEUE.get_or_init(|| {
        let queue = Arc::new(ProcessingStartQueue::new(ASYNC_PROCESSING_QUEUE_DEPTH));
        let callback = Arc::new(start_processing_message);
        for _ in 0..ASYNC_PROCESSING_WORKER_COUNT {
            let queue = Arc::clone(&queue);
            let callback = Arc::clone(&callback);
            thread::spawn(move || {
                while let Some(task) = queue.next() {
                    callback(&task.context, &task.message_id);
                }
            });
        }
        queue
    });
    queue.submit(ProcessingStartTask {
        context: context.clone(),
        message_id: message_id.to_owned(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn task(message_id: &str) -> ProcessingStartTask {
        ProcessingStartTask {
            context: RemoteCliContext::new(
                PathBuf::from("config.toml"),
                PathBuf::from("axon"),
                axon_config::RemoteFeishuWebSocketConfig::default(),
            ),
            message_id: message_id.to_owned(),
        }
    }

    fn queued_ids(queue: &ProcessingStartQueue) -> Vec<String> {
        queue
            .state
            .lock()
            .expect("队列锁")
            .iter()
            .map(|item| item.message_id.clone())
            .collect()
    }

    #[test]
    fn duplicate_message_refreshes_to_lru_tail() {
        let queue = ProcessingStartQueue::new(3);
        queue.submit(task("message-a"));
        queue.submit(task("message-b"));
        queue.submit(task("message-a"));
        assert_eq!(
            queued_ids(&queue),
            vec!["message-b".to_owned(), "message-a".to_owned()]
        );
    }

    #[test]
    fn full_queue_evicts_oldest_entry() {
        let queue = ProcessingStartQueue::new(2);
        queue.submit(task("message-a"));
        queue.submit(task("message-b"));
        queue.submit(task("message-c"));
        assert_eq!(
            queued_ids(&queue),
            vec!["message-b".to_owned(), "message-c".to_owned()]
        );
    }

    #[test]
    fn processing_worker_count_is_bounded() {
        assert_eq!(ASYNC_PROCESSING_WORKER_COUNT, 4);
        assert_eq!(ASYNC_PROCESSING_QUEUE_DEPTH, 32);
    }
}
