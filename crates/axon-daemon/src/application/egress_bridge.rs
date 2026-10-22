//! 已提交事件的统一出站桥。
//!
//! Durable `EventPublisher` 是唯一的外部持久投影入口；实时 Gateway/TUI/WS
//! 订阅只是同一事件的非 durable 投影。桥先调用 durable publisher，成功后才
//! 发布到内存流，避免订阅者看到尚未提交的事件，也避免渠道各自复制顺序逻辑。

use super::event_stream_bridge::EventStreamBridge;
use axon_domain::{ConsumerCursor, DaemonSessionId, InteractionId, RuntimeObservationFrame};
use axon_session::SessionEventRecord;
use axon_session_runtime::EventPublisher;

/// 统一的已提交事件出站桥。
pub(crate) struct CommittedEventEgress<'a, P> {
    publisher: &'a P,
    stream_bridge: &'a EventStreamBridge,
}

/// Execution Worker 使用的拥有型适配器；与借用型桥保持完全相同的顺序语义。
#[derive(Clone)]
pub(crate) struct CommittedEventPublisher<P> {
    publisher: P,
    stream_bridge: std::sync::Arc<EventStreamBridge>,
}

impl<P> CommittedEventPublisher<P> {
    /// 绑定既有 publisher 和实时桥。
    pub(crate) fn new(publisher: P, stream_bridge: std::sync::Arc<EventStreamBridge>) -> Self {
        Self {
            publisher,
            stream_bridge,
        }
    }
}

impl<P> EventPublisher for CommittedEventPublisher<P>
where
    P: EventPublisher,
{
    fn publish(&self, event: &SessionEventRecord, cursor: &ConsumerCursor) -> Result<(), String> {
        self.publisher.publish(event, cursor)?;
        self.stream_bridge.publish(event, cursor);
        Ok(())
    }

    fn publish_observation(&self, frame: &RuntimeObservationFrame) -> Result<(), String> {
        self.publisher.publish_observation(frame)?;
        if let Ok(session_id) = DaemonSessionId::new(&frame.session_id) {
            self.stream_bridge.publish_observation(&session_id, frame);
        } else {
            self.stream_bridge.broadcast_observation(frame);
        }
        Ok(())
    }
}

impl<'a, P> CommittedEventEgress<'a, P> {
    /// 绑定既有 durable publisher 与实时事件桥，不创建新的存储或队列。
    pub(crate) fn new(publisher: &'a P, stream_bridge: &'a EventStreamBridge) -> Self {
        Self {
            publisher,
            stream_bridge,
        }
    }
}

impl<P> CommittedEventEgress<'_, P>
where
    P: EventPublisher,
{
    /// 先持久发布，再通知实时订阅者。
    pub(crate) fn publish(
        &self,
        event: &SessionEventRecord,
        cursor: &ConsumerCursor,
    ) -> Result<(), String> {
        self.publisher.publish(event, cursor)?;
        self.stream_bridge.publish(event, cursor);
        Ok(())
    }

    /// 先持久发布，再通知实时订阅者；调用方已知 InteractionId 时跳过 JSON 解析。
    pub(crate) fn publish_for_interaction(
        &self,
        interaction_id: &InteractionId,
        event: &SessionEventRecord,
        cursor: &ConsumerCursor,
    ) -> Result<(), String> {
        self.publisher.publish(event, cursor)?;
        self.stream_bridge
            .publish_for_interaction(interaction_id, event, cursor);
        Ok(())
    }

    /// 发布非持久观测帧给实时订阅者。
    pub(crate) fn publish_observation(
        &self,
        frame: &RuntimeObservationFrame,
    ) -> Result<(), String> {
        self.publisher.publish_observation(frame)?;
        if let Ok(session_id) = DaemonSessionId::new(&frame.session_id) {
            self.stream_bridge.publish_observation(&session_id, frame);
        } else {
            self.stream_bridge.broadcast_observation(frame);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::DaemonSessionId;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingPublisher(Arc<Mutex<Vec<String>>>);

    impl EventPublisher for RecordingPublisher {
        fn publish(
            &self,
            event: &SessionEventRecord,
            _cursor: &ConsumerCursor,
        ) -> Result<(), String> {
            self.0
                .lock()
                .expect("测试 publisher 锁")
                .push(event.event_type.clone());
            Ok(())
        }
    }

    #[test]
    fn durable_publish_precedes_realtime_projection() {
        let publisher = RecordingPublisher::default();
        let stream_bridge = EventStreamBridge::new();
        let session = DaemonSessionId::new("egress-session").expect("Session ID");
        let interaction =
            axon_domain::InteractionId::new("egress-interaction").expect("Interaction ID");
        let subscription = stream_bridge
            .subscribe(session.clone(), interaction, None)
            .expect("订阅");
        let event = SessionEventRecord {
            session_id: session,
            sequence: 1,
            emitted_at_ms: 1,
            generation: "generation-a".to_owned(),
            event_type: "turn_completed".to_owned(),
            payload_json: r#"{"interactionId":"egress-interaction","response":"done"}"#.to_owned(),
        };
        CommittedEventEgress::new(&publisher, &stream_bridge)
            .publish(&event, &ConsumerCursor::new("generation-a", 1))
            .expect("出站");
        assert_eq!(
            publisher.0.lock().expect("读取 publisher").as_slice(),
            ["turn_completed"]
        );
        assert!(subscription.try_recv().is_ok());
    }
}
