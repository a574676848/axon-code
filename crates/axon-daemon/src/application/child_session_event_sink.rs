//! 子代理会话的 durable sink：把子代理的逐字稿写进**它自己**的会话事件流。
//!
//! 子代理是独立会话，它的用户消息、助手文本、工具调用与结果都必须像普通会话一样
//! 落进 `session_events`。这样界面不必为它另造一条解析/渲染管线：
//! 用与会话完全相同的投影即可。
//!
//! 写入方仍是持锁的宿主：这里复用宿主已经持有的 host 记录与时钟，
//! 以 host 代际围栏追加（`append_events`，无需 interaction 租约），
//! 不新开第二条持久化路径。

use std::sync::Arc;

use axon_domain::DaemonSessionId;
use axon_session::{DaemonHostRecord, RuntimeStore};
use axon_session_runtime::{RuntimeClock, RuntimeEvent, SubagentSessionSinkFactory, TurnEventSink};

use crate::application::event_stream_bridge::EventStreamBridge;

/// 为子代理会话派生 sink 的工厂。
pub(super) struct ChildSessionSinkFactory<S, C> {
    pub(super) store: S,
    pub(super) host: DaemonHostRecord,
    pub(super) clock: C,
    pub(super) cwd: String,
    /// 实时订阅桥：子会话事件落库后要像主会话一样推给订阅者。
    pub(super) stream_bridge: Arc<EventStreamBridge>,
}

impl<S, C> SubagentSessionSinkFactory for ChildSessionSinkFactory<S, C>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    C: RuntimeClock + Clone + Send + Sync + 'static,
{
    fn sink_for(&self, session_id: &str) -> Result<Arc<dyn TurnEventSink>, String> {
        let session_id = DaemonSessionId::new(session_id)
            .map_err(|error| format!("子代理会话标识非法：{error:?}"))?;
        // 登记（或复用）子会话运行态行：这是「一等会话」的登记面，
        // 同时让宿主代际围栏对这条会话生效。
        self.store
            .create_session(&session_id, &self.cwd, self.clock.now_ms())
            .map_err(|error| format!("登记子代理会话失败：{error:?}"))?;
        Ok(Arc::new(ChildSessionSink {
            store: self.store.clone(),
            host: self.host.clone(),
            session_id,
            clock: self.clock.clone(),
            stream_bridge: Arc::clone(&self.stream_bridge),
        }))
    }
}

/// 单个子代理会话的 durable sink。
struct ChildSessionSink<S, C> {
    store: S,
    host: DaemonHostRecord,
    session_id: DaemonSessionId,
    clock: C,
    stream_bridge: Arc<EventStreamBridge>,
}

impl<S, C> TurnEventSink for ChildSessionSink<S, C>
where
    S: RuntimeStore + Send + Sync,
    C: RuntimeClock + Send + Sync,
{
    fn publish(&self, event: &RuntimeEvent) -> Result<(), String> {
        let persisted = self
            .store
            .append_events(
                &self.session_id,
                &self.host,
                std::slice::from_ref(&(event.event_type.clone(), event.payload_json.clone())),
                self.clock.now_ms(),
            )
            .map_err(|error| format!("子代理会话事件追加失败：{error:?}"))?;
        // 落库之后再发布：订阅者看到的每一帧都已经是 durable 事实。
        for record in &persisted {
            self.stream_bridge.publish(record, &record.cursor());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "child_session_event_sink_tests.rs"]
mod tests;
