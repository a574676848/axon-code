//! 跨进程非持久运行时指标观测帧的发布与订阅端口。

use axon_domain::RuntimeObservationFrame;

/// 跨进程非持久运行时观测帧的发布端口。
pub trait ObservationPublisher: Send + Sync {
    /// 发布非持久观测帧（不落盘、不改变游标、允许丢弃）。
    fn publish_observation(&self, frame: &RuntimeObservationFrame) -> Result<(), String>;
}

/// 运行时观测帧订阅消费端口（供 TUI、CLI、Gateway 等使用）。
pub trait ObservationSubscriber: Send + Sync {
    /// 消费一条到达的非持久观测帧。
    fn on_observation(&self, frame: &RuntimeObservationFrame);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockSubscriber {
        received: Arc<AtomicUsize>,
    }

    impl ObservationSubscriber for MockSubscriber {
        fn on_observation(&self, _frame: &RuntimeObservationFrame) {
            self.received.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct MockPublisher {
        subscribers: Vec<Box<dyn ObservationSubscriber>>,
    }

    impl ObservationPublisher for MockPublisher {
        fn publish_observation(&self, frame: &RuntimeObservationFrame) -> Result<(), String> {
            for sub in &self.subscribers {
                sub.on_observation(frame);
            }
            Ok(())
        }
    }

    #[test]
    fn observation_pubsub_dispatch() {
        let received = Arc::new(AtomicUsize::new(0));
        let subscriber = MockSubscriber {
            received: Arc::clone(&received),
        };
        let publisher = MockPublisher {
            subscribers: vec![Box::new(subscriber)],
        };

        let frame = RuntimeObservationFrame::new(1700000000000, "session-1", "turn-1");
        publisher.publish_observation(&frame).expect("发布成功");
        assert_eq!(received.load(Ordering::SeqCst), 1);
    }
}
