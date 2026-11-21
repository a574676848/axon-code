//! 子代理会话的 durable sink：把子代理的逐字稿写进**它自己**的会话事件流。
//!
//! 子代理是独立会话，它的用户消息、助手文本、工具调用与结果都必须像普通会话一样
//! 落进 `session_events`。这样界面不必为它另造一条解析/渲染管线：
//! 用与会话完全相同的投影即可。
//!
//! 归属（INV-4）：子代理的帧必须带派发轮次归属。`SubagentSessionIdentity` 只有会话、
//! 根会话与代理路径，**没有**派发交互标识，因此这里在构造 sink 时按派发行反查：把
//! `inbox.session_id = 子会话` 且来源键带 `agent-dispatch:` 前缀的那一行认成归属，
//! 并改走 `append_attributed_events`（该入口允许「有归属、无租约」：子会话的轮次在
//! 收口之后才写逐字稿的情形很常见）。
//!
//! 查不到归属时不得静默落无归属：留结构化日志与计数器，并让 `wire_event` 在存储层
//! 被拒绝（会话级事件仍然可写）。派发行晚于首批帧到达时，这里会在下一次写入前重试，
//! 补上归属后恢复正常。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axon_domain::{AGENT_DISPATCH_SOURCE_PREFIX, DaemonSessionId, InteractionId};
use axon_session::{
    DaemonHostRecord, NewSessionRequest, RuntimeStore, SessionKind, SessionTrustScope,
};
use axon_session_runtime::{
    RuntimeClock, RuntimeEvent, SubagentSessionIdentity, SubagentSessionSinkFactory, TurnEventSink,
};

use crate::application::event_stream_bridge::EventStreamBridge;

/// 反查派发行的扫描上限：一条子会话的轮次不会很多，取一页足够找到最新一次派发。
const DISPATCH_SCAN_LIMIT: u32 = 64;

/// 无派发归属的子代理事件写入次数。
///
/// 非零即说明派发行缺失或晚到：`wire_event` 会被存储层拒绝，逐字稿因此不会静默落成
/// 无归属帧。计数器只增不清，查不到归属时随告警一并报出，测试也据此断言「没有静默」。
static UNATTRIBUTED_SUBAGENT_WRITES: AtomicU64 = AtomicU64::new(0);

/// 读取累计的无归属写入次数。
#[cfg(test)]
#[must_use]
pub(in crate::application) fn unattributed_subagent_writes() -> u64 {
    UNATTRIBUTED_SUBAGENT_WRITES.load(Ordering::Acquire)
}

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
        let attribution = resolve_dispatch_interaction(&self.store, &session_id);
        if attribution.is_none() {
            // 明确记录而不是静默降级：没有派发行时，逐字稿的 `wire_event` 会被存储层
            // 拒绝，必须让人能从这里直接指回是哪条子会话缺了派发。
            axon_logger::warning(&format!(
                "[child-sink] session={} root={} agent_path={} 查不到派发轮次归属（累计无归属写入 {} 次）：子代理事件将按会话级入口写入并在 wire_event 上被拒绝",
                session_id.as_str(),
                identity.root_session_id,
                identity.agent_path,
                UNATTRIBUTED_SUBAGENT_WRITES.load(Ordering::Acquire)
            ));
        }
        Ok(Arc::new(ChildSessionSink {
            store: self.store.clone(),
            host: self.host.clone(),
            session_id,
            clock: self.clock.clone(),
            stream_bridge: Arc::clone(&self.stream_bridge),
            attribution: Mutex::new(attribution),
        }))
    }
}

/// 解析这条子会话当前的派发轮次。
///
/// 同一子会话被反复派发时取最新一条：sink 在这一轮开始时构造，最新的派发就是这一轮。
/// 查询失败按「查不到」处理并留告警：逐字稿写入是否放行不能靠一次读失败来猜。
fn resolve_dispatch_interaction(
    store: &impl RuntimeStore,
    session_id: &DaemonSessionId,
) -> Option<InteractionId> {
    match store.interactions_for_session(session_id.as_str(), DISPATCH_SCAN_LIMIT, None) {
        Ok(rows) => rows
            .into_iter()
            .rfind(|row| {
                row.source
                    .source_message_id()
                    .starts_with(AGENT_DISPATCH_SOURCE_PREFIX)
            })
            .map(|row| row.interaction_id),
        Err(error) => {
            axon_logger::warning(&format!(
                "[child-sink] session={} 反查派发归属失败：{error}",
                session_id.as_str()
            ));
            None
        }
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
    /// 派发轮次归属；为空表示还没查到（派发行可能晚到，写入前会重试）。
    attribution: Mutex<Option<InteractionId>>,
}

impl<S, C> ChildSessionSink<S, C>
where
    S: RuntimeStore + Send + Sync,
    C: RuntimeClock + Send + Sync,
{
    /// 取本轮归属；缓存为空时先重试一次反查，以便派发行晚到时自愈。
    fn attribution(&self) -> Option<InteractionId> {
        let mut cached = self
            .attribution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cached.is_none()
            && let Some(resolved) = resolve_dispatch_interaction(&self.store, &self.session_id)
        {
            axon_logger::info(&format!(
                "[child-sink] session={} 补上派发归属 interaction={}",
                self.session_id.as_str(),
                resolved.as_str()
            ));
            *cached = Some(resolved);
        }
        cached.clone()
    }
}

impl<S, C> TurnEventSink for ChildSessionSink<S, C>
where
    S: RuntimeStore + Send + Sync,
    C: RuntimeClock + Send + Sync,
{
    fn publish(&self, event: &RuntimeEvent) -> Result<(), String> {
        let payloads = [(event.event_type.clone(), event.payload_json.clone())];
        let now_ms = self.clock.now_ms();
        let persisted = match self.attribution() {
            Some(interaction_id) => self.store.append_attributed_events(
                &self.session_id,
                &self.host,
                &interaction_id,
                &payloads,
                now_ms,
            ),
            None => {
                // 无归属只允许会话级事件：`wire_event` 会在存储层被拒绝，逐字稿因此
                // 不会静默落成「没有轮次」的孤儿帧。计数器与告警在构造 sink 时已留痕。
                UNATTRIBUTED_SUBAGENT_WRITES.fetch_add(1, Ordering::AcqRel);
                self.store
                    .append_events(&self.session_id, &self.host, &payloads, now_ms)
            }
        }
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
