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
use axon_session::{
    DaemonHostRecord, NewSessionRequest, RuntimeStore, SessionKind, SessionTrustScope,
};
use axon_session_runtime::{
    RuntimeClock, RuntimeEvent, SubagentSessionIdentity, SubagentSessionSinkFactory, TurnEventSink,
};

use crate::application::event_stream_bridge::EventStreamBridge;

/// 为子代理会话派生 sink 的工厂。
pub(super) struct ChildSessionSinkFactory<S, C> {
    pub(super) store: S,
    pub(super) host: DaemonHostRecord,
    pub(super) clock: C,
    /// 实时订阅桥：子会话事件落库后要像主会话一样推给订阅者。
    pub(super) stream_bridge: Arc<EventStreamBridge>,
}

impl<S, C> SubagentSessionSinkFactory for ChildSessionSinkFactory<S, C>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    C: RuntimeClock + Clone + Send + Sync + 'static,
{
    fn sink_for(
        &self,
        identity: &SubagentSessionIdentity,
    ) -> Result<Arc<dyn TurnEventSink>, String> {
        let session_id = DaemonSessionId::new(identity.session_id.as_str())
            .map_err(|error| format!("子代理会话标识非法：{error:?}"))?;
        ensure_subagent_session(&self.store, self.clock.now_ms(), identity)?;
        Ok(Arc::new(ChildSessionSink {
            store: self.store.clone(),
            host: self.host.clone(),
            session_id,
            clock: self.clock.clone(),
            stream_bridge: Arc::clone(&self.stream_bridge),
        }))
    }
}

/// 确保子代理会话已登记。
///
/// 子代理与普通会话同为一等会话：标识、类型、根会话与代理路径都由派发方给出，
/// 这里只在真的缺行时补登记。根会话已登记时，受众边界与工作区从它继承，
/// 因此子会话与父会话属于同一个 Workspace、同一条披露边界。
///
/// 模型子代理与后台执行两个 sink 工厂共用这一套登记口径：落点都是 turn 所属子会话。
pub(in crate::application::daemon_host) fn ensure_subagent_session<S>(
    store: &S,
    now_ms: u64,
    identity: &SubagentSessionIdentity,
) -> Result<(), String>
where
    S: RuntimeStore,
{
    let session_id = DaemonSessionId::new(identity.session_id.as_str())
        .map_err(|error| format!("子代理会话标识非法：{error:?}"))?;
    if store.get_session(&session_id).is_ok() {
        return Ok(());
    }
    let root_session_id = DaemonSessionId::new(identity.root_session_id.as_str())
        .map_err(|error| format!("子代理根会话标识非法：{error:?}"))?;
    let root = store.get_session(&root_session_id).ok();
    // 执行子会话要说清自己属于哪个目标：目标标识从根会话继承，执行标识由调度侧另行回填。
    let goal_id = store.session_goal_id(&root_session_id).ok().flatten();
    let request = NewSessionRequest {
        session_id,
        kind: SessionKind::Subagent,
        trust_scope: root
            .as_ref()
            .map_or(SessionTrustScope::Private, |record| record.trust_scope),
        workspace_id: root.as_ref().and_then(|record| record.workspace_id.clone()),
        root_session_id: Some(root_session_id),
        agent_path: Some(identity.agent_path.clone()),
        goal_id,
        execution_id: None,
        cwd: root
            .as_ref()
            .map(|record| record.cwd.clone())
            .unwrap_or_default(),
        now_ms,
    };
    store
        .create_session_with(&request)
        .map(|_| ())
        .map_err(|error| format!("登记子代理会话失败：{error:?}"))
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
