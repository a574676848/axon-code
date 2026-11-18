//! 将 Gateway Kernel 接到既有 Daemon durable ingress 与事件桥。

#[path = "gateway_event_options.rs"]
mod event_options;
use event_options::prepare_event_content;
pub use event_options::{EVENT_CONTENT_INLINE_LIMIT_CHARS, GatewayEventOptions};

use super::gateway_debounce::GatewayDebouncer;
#[path = "gateway_session_cache.rs"]
mod session_cache;
use super::gateway_kernel::{
    GatewayEgress, GatewayEvent, GatewayEventStream, GatewayIngress, GatewayKernelError,
    GatewaySubscriptionRequest,
};
use super::gateway_replay::{
    DEFAULT_GATEWAY_REPLAY_PAGE_SIZE, interaction_status_event, live_ceiling_cursor,
    subscribe_and_replay, subscribe_and_replay_session,
};
use crate::application::event_stream_bridge::STREAM_RESET_REASON_GENERATION_CHANGE;
use crate::application::{
    EventStreamBridge, EventStreamSubscription, ExternalEventEnvelope, ExternalEventIngress,
    GatewayHostDependencies, StreamEvent, StreamNotification,
};
use axon_domain::{ConsumerCursor, DaemonSessionId, InteractionId};
use axon_protocol::SubmitInteractionReceipt;
use axon_session::{RuntimeStore, WorkspaceResolver};
use axon_session_runtime::RuntimeClock;
use session_cache::{SessionAdmission, SessionAdmissionCache};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

const GATEWAY_ADAPTER_ID: &str = "gateway";
/// 网关外部事件的固定会话标识。
///
/// 外部 webhook 无从预知服务端生成的会话标识，因此这类事件统一落到这一条固定会话：
/// 首次到达时登记，此后所有外部事件复用它。标识是一次性常量，不参与任何派生。
const GATEWAY_EVENT_SESSION_ID: &str = "01J8XQ7K3M4N5P6Q7R8S9T0V1W";
/// 放行缓存的容量：单进程同时活跃的会话远小于这个数。
const ADMISSION_CACHE_CAPACITY: usize = 1_024;
/// 放行缓存的存活时间：会话归属与受众边界建行后不变，缓存只需挡住抖动。
const ADMISSION_CACHE_TTL_MS: u64 = 30_000;

/// Daemon Gateway 的统一 ingress/egress 实现。
///
/// 该桥只持有现有 Host 的依赖快照，不创建新的存储、队列或执行 Worker。
#[derive(Clone)]
pub struct DaemonGatewayBridge<S, C> {
    dependencies: GatewayHostDependencies<S, C>,
    replay_page_size: u32,
    default_workspace_id: Option<String>,
    events: Option<GatewayEventOptions>,
    debouncer: Option<Arc<GatewayDebouncer>>,
    /// 会话放行缓存：入站与订阅共用，避免每次入站都回表查一次主键。
    admissions: Arc<SessionAdmissionCache>,
}

impl<S: RuntimeStore, C> DaemonGatewayBridge<S, C> {
    /// 使用 DaemonHost 的依赖快照创建 bridge。
    #[must_use]
    pub fn new(dependencies: GatewayHostDependencies<S, C>) -> Self {
        Self::build(dependencies, DEFAULT_GATEWAY_REPLAY_PAGE_SIZE, None)
    }

    /// 创建带有界 durable replay 页大小的 bridge。
    #[must_use]
    pub fn new_with_replay_page_size(
        dependencies: GatewayHostDependencies<S, C>,
        replay_page_size: u32,
    ) -> Self {
        Self::build(dependencies, replay_page_size, None)
    }

    /// 创建带事件增强（provider 内容资产、正文归档、滑动窗口防抖）的 bridge。
    #[must_use]
    pub fn new_with_events(
        dependencies: GatewayHostDependencies<S, C>,
        options: GatewayEventOptions,
    ) -> Self {
        Self::build(
            dependencies,
            DEFAULT_GATEWAY_REPLAY_PAGE_SIZE,
            Some(options),
        )
    }

    fn build(
        dependencies: GatewayHostDependencies<S, C>,
        replay_page_size: u32,
        events: Option<GatewayEventOptions>,
    ) -> Self {
        let debouncer = events
            .as_ref()
            .map(|options| Arc::new(GatewayDebouncer::new(options.debounce_window)));
        Self {
            dependencies,
            replay_page_size: replay_page_size.max(1),
            default_workspace_id: resolve_default_workspace_id(),
            events,
            debouncer,
            admissions: Arc::new(SessionAdmissionCache::new(
                ADMISSION_CACHE_CAPACITY,
                ADMISSION_CACHE_TTL_MS,
            )),
        }
    }

    /// 取一条会话的放行判定：先看缓存，未命中才查一次主键并记下。
    fn admission_of(
        &self,
        session_id: &DaemonSessionId,
        now_ms: u64,
    ) -> Result<SessionAdmission, GatewayKernelError> {
        if let Some(admission) = self.admissions.get(session_id.as_str(), now_ms) {
            return Ok(admission);
        }
        let session = self
            .dependencies
            .store
            .get_session(session_id)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        let admission = SessionAdmission {
            workspace_id: session.workspace_id,
            trust_scope: session.trust_scope,
        };
        self.admissions.put(session_id.as_str(), &admission, now_ms);
        Ok(admission)
    }

    /// 取（必要时登记）网关外部事件的固定会话标识。
    ///
    /// 外部事件没有会话标识可携带，也不能按投递目标派生：它们全部落在一条固定会话上，
    /// 首次到达时创建，此后复用，因此 webhook 历史始终连续。放行判定走缓存：这条会话
    /// 一旦登记就不再变化，后续入站不必每帧回表。
    ///
    /// 这条会话**不绑定任何项目 Workspace**：外部事件的处理根固定是 Axon 根目录
    /// `~/.axon`（provider 脚本在 `~/.axon/events`、正文归档在 `~/.axon/data`），
    /// 事件不属于任何项目工作区，因此 `workspace_id` 留空、`cwd` 也留空。
    fn gateway_event_session(&self, now_ms: u64) -> Result<DaemonSessionId, GatewayKernelError> {
        let session_id = DaemonSessionId::new(GATEWAY_EVENT_SESSION_ID)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        if self.admissions.get(session_id.as_str(), now_ms).is_some() {
            return Ok(session_id);
        }
        let session = match self.dependencies.store.get_session(&session_id) {
            Ok(session) => session,
            Err(_) => self
                .dependencies
                .store
                .create_session_with(&axon_session::NewSessionRequest {
                    session_id: session_id.clone(),
                    kind: axon_session::SessionKind::Primary,
                    trust_scope: axon_session::SessionTrustScope::Private,
                    workspace_id: None,
                    root_session_id: None,
                    agent_path: None,
                    goal_id: None,
                    execution_id: None,
                    cwd: String::new(),
                    now_ms,
                })
                .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?,
        };
        self.admissions.put(
            session_id.as_str(),
            &SessionAdmission {
                workspace_id: session.workspace_id,
                trust_scope: session.trust_scope,
            },
            now_ms,
        );
        Ok(session_id)
    }

    fn absorb_debounced(
        &self,
        correlation_key: Option<&str>,
        now_ms: u64,
    ) -> Option<SubmitInteractionReceipt> {
        let (debouncer, key) = (self.debouncer.as_ref()?, correlation_key?);
        debouncer.absorb(key, now_ms)
    }
}

impl<S, C> GatewayIngress for DaemonGatewayBridge<S, C>
where
    S: RuntimeStore + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
{
    fn submit_external_event(
        &self,
        mut event: ExternalEventEnvelope,
    ) -> Result<SubmitInteractionReceipt, GatewayKernelError> {
        // 外部事件不对外暴露 Workspace 与会话：Workspace 固定取网关默认（Axon 根目录），
        // 会话固定取内部常量。
        event.workspace_id = self.default_workspace_id.clone().ok_or_else(|| {
            GatewayKernelError::Ingress("无法解析 Gateway 默认 Workspace".to_owned())
        })?;
        let now_ms = self.dependencies.clock.now_ms();
        // 会话标识：显式给出时必须在控制面已登记且归属一致；未给出时（外部 webhook 无从
        // 预知标识）解析该 Workspace 的活跃会话，没有才新建一条主会话并继续投递。
        let session_id = match event.session_id.clone() {
            Some(session_id) => {
                let admission = self.admission_of(&session_id, now_ms)?;
                if admission
                    .workspace_id
                    .as_deref()
                    .is_some_and(|workspace| workspace != event.workspace_id.as_str())
                {
                    return Err(GatewayKernelError::Ingress(
                        "会话不属于该 Workspace".to_owned(),
                    ));
                }
                session_id
            }
            None => self.gateway_event_session(now_ms)?,
        };
        event.session_id = Some(session_id);
        let debounce_key = event
            .correlation_key
            .clone()
            .filter(|key| !key.trim().is_empty());
        if let Some(receipt) = self.absorb_debounced(debounce_key.as_deref(), now_ms) {
            return Ok(receipt);
        }
        if let Some(options) = self.events.as_ref() {
            prepare_event_content(&mut event, options)?;
        }
        let ingress = ExternalEventIngress::new_unbound_with_clock_skew(
            self.dependencies.store.clone(),
            Arc::clone(&self.dependencies.active_turn_steer_sink),
            GATEWAY_ADAPTER_ID,
            now_ms,
            self.dependencies.action_lease_duration_ms,
            crate::application::DEFAULT_EXTERNAL_EVENT_MAX_CLOCK_SKEW_MS,
        )
        .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        let receipt = ingress
            .submit(event)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        // 只有真正落 durable 的新事件才唤醒主 turn：防抖吸收与重复事件不产生新工作。
        if receipt.inserted
            && let Some(options) = self.events.as_ref()
        {
            options.wake();
        }
        if let (Some(debouncer), Some(key)) = (self.debouncer.as_ref(), debounce_key.as_deref()) {
            debouncer.register(key, now_ms, receipt.interaction_id.clone());
        }
        Ok(SubmitInteractionReceipt {
            interaction_id: receipt.interaction_id,
            accepted: true,
            deduplicated: !receipt.inserted,
        })
    }

    fn get_interaction_response(
        &self,
        interaction_id: &InteractionId,
    ) -> Result<Option<super::gateway_kernel::GatewayInteractionResponse>, GatewayKernelError> {
        let Some(interaction) = self
            .dependencies
            .store
            .get_interaction(interaction_id)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?
        else {
            return Ok(None);
        };
        let response = self.extract_interaction_response(&interaction);
        let status = serde_json::to_value(interaction.status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        Ok(Some(super::gateway_kernel::GatewayInteractionResponse {
            interaction_id: interaction.interaction_id.as_str().to_owned(),
            status,
            received_at_ms: interaction.received_at_ms,
            channel: interaction.source.channel().to_owned(),
            content: interaction.content,
            response,
        }))
    }
}

impl<S, C> DaemonGatewayBridge<S, C>
where
    S: RuntimeStore + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
{
    fn extract_interaction_response(
        &self,
        interaction: &axon_domain::InteractionRecord,
    ) -> Option<String> {
        let session_id =
            crate::application::primary_session_id_for_interaction(interaction).ok()?;
        let events = self
            .dependencies
            .store
            .replay_events(&session_id, 0, 100)
            .ok()?;
        let interaction_id_str = interaction.interaction_id.as_str();
        events
            .iter()
            .rev()
            .find(|event| {
                event.event_type == axon_session_runtime::model::TURN_COMPLETED_EVENT
                    && serde_json::from_str::<serde_json::Value>(&event.payload_json)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("interactionId")
                                .and_then(serde_json::Value::as_str)
                                .map(|id| id == interaction_id_str)
                        })
                        .unwrap_or(false)
            })
            .and_then(|event| serde_json::from_str::<serde_json::Value>(&event.payload_json).ok())
            .and_then(|value| {
                value
                    .get("response")
                    .and_then(serde_json::Value::as_str)
                    .map(|text| text.trim().to_owned())
            })
            .filter(|text| !text.is_empty())
    }
}

impl<S, C> GatewayEgress for DaemonGatewayBridge<S, C>
where
    S: RuntimeStore + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
{
    fn subscribe(
        &self,
        request: GatewaySubscriptionRequest,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
        let workspace_id = non_empty(request.workspace_id, "workspace_id")?;
        // 会话级订阅：客户端直接给出会话（子代理会话只能由主 turn 的子代理区域进入），
        // 因此这里校验「会话存在」与「它是本工作区某条车道会话派生出来的子会话」，
        // 而不是绕过 Interaction 校验去订阅任意会话。
        if let Some(session_id) = request.session_id.clone() {
            return self.subscribe_session_stream(
                &workspace_id,
                &session_id,
                request.resume_cursor,
            );
        }
        let interaction_id = InteractionId::new(request.interaction_id)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        let interaction = self
            .dependencies
            .store
            .get_interaction(&interaction_id)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?
            .ok_or_else(|| GatewayKernelError::Ingress("订阅目标 Interaction 不存在".to_owned()))?;
        let interaction_workspace = interaction
            .scope
            .workspace_id()
            .ok_or_else(|| GatewayKernelError::Ingress("订阅目标必须绑定 Workspace".to_owned()))?;
        if interaction_workspace.as_str() != workspace_id {
            return Err(GatewayKernelError::Ingress(
                "订阅 Workspace 与 Interaction scope 不匹配".to_owned(),
            ));
        }
        let session_id = crate::application::primary_session_id_for_interaction(&interaction)
            .map_err(GatewayKernelError::Ingress)?;
        // durable 尾游标先于实时订阅读取；顺序不变量见 `gateway_replay` 模块文档。
        let live_ceiling = live_ceiling_cursor(
            &self.dependencies.store,
            &session_id,
            self.dependencies.host_generation.as_deref(),
        )?;
        let cursor = request
            .resume_cursor
            .as_deref()
            .map(parse_cursor)
            .transpose()?;
        let replay_cursor = cursor.filter(|value| {
            self.dependencies
                .host_generation
                .as_ref()
                .is_none_or(|generation| generation == &value.generation)
        });
        let (subscription, pending, replay_cursor) = subscribe_and_replay(
            &self.dependencies.store,
            &self.dependencies.stream_bridge,
            &session_id,
            &interaction_id,
            replay_cursor,
            self.replay_page_size,
            live_ceiling,
        )?;
        Ok(Box::new(DaemonGatewayEventStream {
            store: self.dependencies.store.clone(),
            stream_bridge: self.dependencies.stream_bridge.clone(),
            session_id,
            interaction_id: Some(interaction_id),
            replay_page_size: self.replay_page_size,
            host_generation: self.dependencies.host_generation.clone(),
            subscription,
            pending,
            replay_cursor,
            // 交互级订阅不经过会话放行，关闭时也不需要回收放行缓存。
            admissions: None,
        }))
    }
}

impl<S, C> DaemonGatewayBridge<S, C>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    C: RuntimeClock + Clone + Send + Sync + 'static,
{
    /// 建立一条会话级订阅（不按 Interaction 过滤）。
    ///
    /// 放行条件只看会话本身，不解析任何标识前缀：
    ///
    /// 1. 会话必须已在运行态登记（`sessions` 主键命中）；
    /// 2. 归属 Workspace 与请求一致（未绑定工作区的会话不作限制）；
    /// 3. 受众边界不是受限档。
    ///
    /// 放行判定走放行缓存：会话归属与受众边界建行后不再变化，重复订阅同一会话不必回表；
    /// 订阅关闭时回收这条会话的缓存条目。
    fn subscribe_session_stream(
        &self,
        workspace_id: &str,
        session_id: &str,
        resume_cursor: Option<String>,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
        let session_id = DaemonSessionId::new(session_id)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        // 放行判定走缓存：会话归属与受众边界建行后不变，重复订阅同一会话不必回表。
        let admission = self.admission_of(&session_id, self.dependencies.clock.now_ms())?;
        self.authorize_session(workspace_id, &admission)?;
        let live_ceiling = live_ceiling_cursor(
            &self.dependencies.store,
            &session_id,
            self.dependencies.host_generation.as_deref(),
        )?;
        let cursor = resume_cursor.as_deref().map(parse_cursor).transpose()?;
        let replay_cursor = cursor.filter(|value| {
            self.dependencies
                .host_generation
                .as_ref()
                .is_none_or(|generation| generation == &value.generation)
        });
        let (subscription, pending, replay_cursor) = subscribe_and_replay_session(
            &self.dependencies.store,
            &self.dependencies.stream_bridge,
            &session_id,
            replay_cursor,
            self.replay_page_size,
            live_ceiling,
        )?;
        Ok(Box::new(DaemonGatewayEventStream {
            store: self.dependencies.store.clone(),
            stream_bridge: self.dependencies.stream_bridge.clone(),
            session_id,
            interaction_id: None,
            replay_page_size: self.replay_page_size,
            host_generation: self.dependencies.host_generation.clone(),
            subscription,
            pending,
            replay_cursor,
            // 会话级订阅关闭时回收这条会话的放行缓存。
            admissions: Some(Arc::clone(&self.admissions)),
        }))
    }

    /// 会话订阅的放行判定：会话已登记，且工作区与受众边界匹配。
    fn authorize_session(
        &self,
        workspace_id: &str,
        admission: &SessionAdmission,
    ) -> Result<(), GatewayKernelError> {
        if admission
            .workspace_id
            .as_deref()
            .is_some_and(|owner| owner != workspace_id)
        {
            return Err(rejection("会话与请求工作区不匹配"));
        }
        if admission.trust_scope == axon_session::SessionTrustScope::Restricted {
            return Err(rejection("会话受众边界受限，本层无法证明受众"));
        }
        Ok(())
    }
}

struct DaemonGatewayEventStream<S> {
    store: S,
    stream_bridge: Arc<EventStreamBridge>,
    session_id: axon_domain::DaemonSessionId,
    /// 订阅的 Interaction；`None` 表示会话级订阅（子代理会话没有 Interaction）。
    interaction_id: Option<InteractionId>,
    replay_page_size: u32,
    /// Host 代际；仅用于标注 durable 上界 cursor，不参与序号判定。
    host_generation: Option<String>,
    subscription: EventStreamSubscription,
    pending: VecDeque<GatewayEvent>,
    /// 重放已同步到的 durable 水位；仅用于消除"先订阅、后 replay"竞态产生的重叠事件。
    replay_cursor: Option<ConsumerCursor>,
    /// 会话级订阅持有的放行缓存：关闭订阅即回收这条会话的缓存条目。
    admissions: Option<Arc<SessionAdmissionCache>>,
}

impl<S> Drop for DaemonGatewayEventStream<S> {
    fn drop(&mut self) {
        // 关闭事件是缓存失效点：没人再看这条会话时，不必替它留着放行判定。
        if self.interaction_id.is_none()
            && let Some(admissions) = self.admissions.as_deref()
        {
            admissions.invalidate(self.session_id.as_str());
        }
    }
}

impl<S> DaemonGatewayEventStream<S>
where
    S: RuntimeStore + Clone,
{
    fn reconnect(
        &mut self,
        reason: &str,
        cursor: Option<ConsumerCursor>,
    ) -> Result<(), GatewayKernelError> {
        let resume_cursor = if reason == STREAM_RESET_REASON_GENERATION_CHANGE {
            None
        } else {
            cursor
        };
        // 上界代际取 Host 当前代际；缺失时留空，不影响序号判定。
        let live_ceiling = live_ceiling_cursor(
            &self.store,
            &self.session_id,
            self.host_generation.as_deref(),
        )?;
        let (subscription, pending, replay_cursor) = match self.interaction_id.as_ref() {
            Some(interaction_id) => subscribe_and_replay(
                &self.store,
                &self.stream_bridge,
                &self.session_id,
                interaction_id,
                resume_cursor,
                self.replay_page_size,
                live_ceiling,
            )?,
            None => subscribe_and_replay_session(
                &self.store,
                &self.stream_bridge,
                &self.session_id,
                resume_cursor,
                self.replay_page_size,
                live_ceiling,
            )?,
        };
        self.subscription = subscription;
        self.pending = pending;
        self.replay_cursor = replay_cursor;
        Ok(())
    }
}

impl<S> GatewayEventStream for DaemonGatewayEventStream<S>
where
    S: RuntimeStore + Clone + Send + 'static,
{
    fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<GatewayEvent>, GatewayKernelError> {
        if let Some(event) = self.pending.pop_front() {
            return Ok(Some(event));
        }
        loop {
            match self.subscription.recv_timeout(timeout) {
                Ok(StreamNotification::SessionEvent(event)) => {
                    if self
                        .replay_cursor
                        .as_ref()
                        .is_some_and(|cursor| !event.cursor.advances_durable_from(cursor))
                    {
                        continue;
                    }
                    self.replay_cursor = None;
                    return Ok(Some(session_event(event)));
                }
                Ok(StreamNotification::LiveEvent(live)) => {
                    // 瞬态帧：与本轮 wire 事件同名，客户端沿用同一套投影；
                    // 没有 cursor 表示它不推进重放水位。
                    return Ok(Some(GatewayEvent {
                        event_type: live.event_type,
                        payload_json: live.payload_json,
                        cursor: None,
                        replay: false,
                    }));
                }
                Ok(StreamNotification::InteractionStatus(status)) => {
                    return Ok(Some(interaction_status_event(status, false)));
                }
                Ok(StreamNotification::Observation(frame)) => {
                    let payload_json =
                        serde_json::to_string(&frame).unwrap_or_else(|_| "{}".to_owned());
                    return Ok(Some(GatewayEvent {
                        event_type: "runtime_observation".to_owned(),
                        payload_json,
                        cursor: None,
                        replay: false,
                    }));
                }
                Ok(StreamNotification::StreamReset { cursor, reason }) => {
                    self.reconnect(reason, cursor)?;
                    if let Some(event) = self.pending.pop_front() {
                        return Ok(Some(event));
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(GatewayKernelError::Ingress(
                        "Daemon 实时事件订阅已断开".to_owned(),
                    ));
                }
            }
        }
    }
}

fn session_event(event: StreamEvent) -> GatewayEvent {
    GatewayEvent {
        event_type: event.event.event_type,
        payload_json: event.event.payload_json,
        cursor: serde_json::to_string(&event.cursor).ok(),
        replay: false,
    }
}

/// 解析客户端回传的 durable cursor；同时是 gateway 事件 cursor 的编解码基准。
pub(super) fn parse_cursor(value: &str) -> Result<ConsumerCursor, GatewayKernelError> {
    serde_json::from_str(value)
        .map_err(|error| GatewayKernelError::Ingress(format!("resume_cursor 无效：{error}")))
}

fn non_empty(value: String, field: &str) -> Result<String, GatewayKernelError> {
    let value = value.trim().to_owned();
    (!value.is_empty())
        .then_some(value)
        .ok_or_else(|| GatewayKernelError::Ingress(format!("{field} 不能为空")))
}

/// 订阅拒绝：客户端只看到 `subscription_rejected`，具体原因写进 Daemon 日志。
fn rejection(reason: &str) -> GatewayKernelError {
    GatewayKernelError::Ingress(reason.to_owned())
}

/// 根目录归一：统一分隔符并去掉尾部分隔符。
///
/// 只做路径形态归一，不解析符号链接，也不做大小写折叠：注册表里的根目录来自
/// `WorkspaceResolver` 的规范化结果，两侧同源才谈得上「精确匹配」。
/// 根目录归一：统一分隔符并去掉尾部分隔符。
///
/// 只做路径形态归一，不解析符号链接，也不做大小写折叠：注册表里的根目录来自
/// `WorkspaceResolver` 的规范化结果，两侧同源才谈得上「精确匹配」。
/// 订阅校验与目标看板的工作区解析共用这一份，避免两处各写一套比较口径。
#[must_use]
pub fn normalize_workspace_root(value: &str) -> String {
    let unified = value.trim().replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    trimmed.to_owned()
}

fn resolve_default_workspace_id() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    WorkspaceResolver
        .resolve(&cwd, 1)
        .ok()
        .map(|workspace| workspace.workspace_id)
}

#[cfg(test)]
#[path = "gateway_bridge_tests.rs"]
mod gateway_bridge_tests;

#[cfg(test)]
#[path = "gateway_bridge_goal_session_tests.rs"]
mod gateway_bridge_goal_session_tests;

#[cfg(test)]
#[path = "gateway_event_tests.rs"]
mod gateway_event_tests;
