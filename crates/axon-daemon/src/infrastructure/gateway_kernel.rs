//! Daemon Gateway 内核：只负责适配器编排、生命周期和有界资源。
//!
//! Gateway 内核不解析具体传输协议，也不拥有 Interaction、Task、Execution
//! 或 Delivery 存储。Adapter 必须通过注入的 [`GatewayIngress`] 写入既有
//! durable ingress，再通过 [`GatewayEgress`] 订阅既有事件桥。

use crate::application::ExternalEventEnvelope;
use crate::infrastructure::gateway_auth::GatewayAuth;
use axon_protocol::SubmitInteractionReceipt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, AtomicUsize, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
};
use std::time::Duration;
use thiserror::Error;

const STATE_STOPPED: u8 = 0;
const STATE_RUNNING: u8 = 1;
const STATE_DRAINING: u8 = 2;
const DEFAULT_CONNECTION_LIMIT: usize = 256;

/// Gateway 运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayLifecycle {
    /// 尚未启动。
    Stopped,
    /// 接受新连接和新事件。
    Running,
    /// 正在排空，只允许已建立连接完成工作。
    Draining,
}

impl GatewayLifecycle {
    fn from_u8(value: u8) -> Self {
        match value {
            STATE_RUNNING => Self::Running,
            STATE_DRAINING => Self::Draining,
            _ => Self::Stopped,
        }
    }
}

/// Gateway 内核错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GatewayKernelError {
    /// Adapter 标识为空。
    #[error("Gateway Adapter ID 不能为空")]
    EmptyAdapterId,
    /// Adapter 标识已注册。
    #[error("Gateway Adapter 已注册：{0}")]
    DuplicateAdapter(String),
    /// 当前生命周期不允许操作。
    #[error("Gateway 当前状态不允许操作：{0:?}")]
    InvalidLifecycle(GatewayLifecycle),
    /// 连接数达到有界上限。
    #[error("Gateway 连接数已达到上限：{0}")]
    ConnectionLimitReached(usize),
    /// Adapter 启停失败。
    #[error("Gateway Adapter {adapter_id} 操作失败：{message}")]
    Adapter {
        /// 失败的 Adapter 标识。
        adapter_id: String,
        /// Adapter 返回的错误信息。
        message: String,
    },
    /// Adapter ingress 失败。
    #[error("Gateway ingress 失败：{0}")]
    Ingress(String),
    /// 事件进入 durable ingress 之前的预处理失败（正文归档等）。
    #[error("Gateway 事件预处理失败：{0}")]
    Preparation(String),
}

/// Adapter 使用的 durable ingress 桥。
pub trait GatewayIngress: Send + Sync {
    /// 校验并 durable append 外部事件；实现必须在返回前完成 append。
    fn submit_external_event(
        &self,
        event: ExternalEventEnvelope,
    ) -> Result<SubmitInteractionReceipt, GatewayKernelError>;

    /// 查询外部交互记录与当前执行结果；默认返回 None。
    fn get_interaction_response(
        &self,
        _interaction_id: &axon_domain::InteractionId,
    ) -> Result<Option<GatewayInteractionResponse>, GatewayKernelError> {
        Ok(None)
    }
}

/// Adapter 使用的事件出口桥。
pub trait GatewayEgress: Send + Sync {
    /// 建立一个由既有 Daemon 事件桥驱动的订阅。
    fn subscribe(
        &self,
        request: GatewaySubscriptionRequest,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError>;
}

/// 事件订阅请求；Workspace/Session 由调用方解析，不在 Kernel 固定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewaySubscriptionRequest {
    /// 事件所属 Workspace。
    pub workspace_id: String,
    /// 目标 Interaction。
    pub interaction_id: String,
    /// 目标会话；给出时按**会话级订阅**处理（子代理会话或目标执行会话，不看 Interaction）。
    pub session_id: Option<String>,
    /// 上次已确认的 durable cursor。
    pub resume_cursor: Option<String>,
}

/// 传输无关的事件流。
pub trait GatewayEventStream: Send {
    /// 在超时内读取一条事件；超时返回 `None`，不产生忙等。
    fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<GatewayEvent>, GatewayKernelError>;
}

/// 由 Daemon durable event bridge 映射出的最小事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayEvent {
    /// 稳定事件类型。
    pub event_type: String,
    /// 已提交事件的 JSON 内容。
    pub payload_json: String,
    /// 可选 durable cursor。
    pub cursor: Option<String>,
    /// 是否来自 durable 历史回放；客户端不得据此触发新消息通知。
    pub replay: bool,
}

/// Gateway 外部交互记录概要与响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayInteractionResponse {
    /// 交互记录标识。
    pub interaction_id: String,
    /// 当前处理状态（received, processing, understood, rejected, interrupted 等）。
    pub status: String,
    /// 接收时间（毫秒）。
    pub received_at_ms: u64,
    /// 来源渠道。
    pub channel: String,
    /// 原始交互正文。
    pub content: String,
    /// 主 turn 执行完成后的最终回复交付文本（尚未完成时为空）。
    pub response: Option<String>,
}

/// Adapter 生命周期上下文。
#[derive(Clone)]
pub struct GatewayContext {
    ingress: Arc<dyn GatewayIngress>,
    egress: Arc<dyn GatewayEgress>,
    connection_limiter: Arc<GatewayConnectionLimiter>,
    auth: Option<Arc<GatewayAuth>>,
    state: Arc<AtomicU8>,
}

impl GatewayContext {
    fn new(
        ingress: Arc<dyn GatewayIngress>,
        egress: Arc<dyn GatewayEgress>,
        connection_limiter: Arc<GatewayConnectionLimiter>,
        state: Arc<AtomicU8>,
        auth: Option<Arc<GatewayAuth>>,
    ) -> Self {
        Self {
            ingress,
            egress,
            connection_limiter,
            auth,
            state,
        }
    }

    /// 提交 durable ingress；排空或停止后拒绝新事件。
    pub fn submit_external_event(
        &self,
        event: ExternalEventEnvelope,
    ) -> Result<SubmitInteractionReceipt, GatewayKernelError> {
        self.ensure_running()?;
        self.ingress.submit_external_event(event)
    }

    /// 建立事件订阅；排空或停止后拒绝新订阅。
    pub fn subscribe(
        &self,
        request: GatewaySubscriptionRequest,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
        self.ensure_running()?;
        self.egress.subscribe(request)
    }

    /// 按稳定 ID 查询外部交互结果；排空或停止后拒绝查询。
    pub fn get_interaction_response(
        &self,
        interaction_id: &axon_domain::InteractionId,
    ) -> Result<Option<GatewayInteractionResponse>, GatewayKernelError> {
        self.ensure_running()?;
        self.ingress.get_interaction_response(interaction_id)
    }

    /// 尝试取得一个连接许可；许可释放后才会减少活动连接计数。
    pub fn try_acquire_connection(&self) -> Result<GatewayConnectionPermit, GatewayKernelError> {
        self.ensure_running()?;
        self.connection_limiter.try_acquire()
    }

    fn ensure_running(&self) -> Result<(), GatewayKernelError> {
        let state = GatewayLifecycle::from_u8(self.state.load(Ordering::Acquire));
        (state == GatewayLifecycle::Running)
            .then_some(())
            .ok_or(GatewayKernelError::InvalidLifecycle(state))
    }

    /// 在 transport 创建连接、mailbox 或订阅资源前校验 Bearer token。
    #[must_use]
    pub fn authorize_bearer(&self, token: &str) -> bool {
        self.auth
            .as_deref()
            .is_some_and(|auth| auth.authorize_bearer(token))
    }

    /// 在 transport 创建连接、mailbox 或订阅资源前校验完整 Authorization header。
    #[must_use]
    pub fn authorize_header(&self, header: Option<&str>) -> bool {
        self.auth
            .as_deref()
            .is_some_and(|auth| auth.authorize_header(header))
    }

    /// 在 transport 创建连接、mailbox 或订阅资源前校验请求凭据。
    ///
    /// `Authorization: Bearer` 与 URL 查询串 token 任一命中即通过；部分发送方
    /// （Jira Server WebHooks 等）无法配置 HTTP header，只能把凭据写在 URL 上。
    #[must_use]
    pub fn authorize_request(&self, header: Option<&str>, query_token: Option<&str>) -> bool {
        self.auth
            .as_deref()
            .is_some_and(|auth| auth.authorize_request(header, query_token))
    }
}

/// Gateway Adapter 实现的最小生命周期契约。
pub trait GatewayAdapter: Send + Sync {
    /// 返回进程内唯一的稳定 Adapter ID。
    fn adapter_id(&self) -> &str;
    /// 启动 Adapter 的接收循环。
    fn start(&self, context: GatewayContext) -> Result<(), String>;
    /// 请求 Adapter 停止接受新连接并排空。
    fn drain(&self);
    /// 释放 Adapter 资源。
    fn stop(&self);
}

/// 有界 Gateway 连接准入器。
#[derive(Debug)]
pub struct GatewayConnectionLimiter {
    limit: usize,
    active: Arc<AtomicUsize>,
}

impl GatewayConnectionLimiter {
    /// 创建连接准入器；零值使用默认 256 上限。
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 使用 IPC 同级的默认连接上限。
    #[must_use]
    pub fn default_limit() -> Self {
        Self::new(DEFAULT_CONNECTION_LIMIT)
    }

    /// 尝试取得连接许可，满载时立即失败，不阻塞调用线程。
    pub fn try_acquire(&self) -> Result<GatewayConnectionPermit, GatewayKernelError> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < self.limit).then_some(current.saturating_add(1))
            })
            .map(|_| GatewayConnectionPermit {
                active: Arc::clone(&self.active),
            })
            .map_err(|_| GatewayKernelError::ConnectionLimitReached(self.limit))
    }

    /// 当前活动连接数。
    #[must_use]
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

/// 一个活动连接许可。
pub struct GatewayConnectionPermit {
    active: Arc<AtomicUsize>,
}

impl Drop for GatewayConnectionPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 固定容量 mailbox；满载时拒绝写入，避免无限堆积。
pub struct GatewayMailbox<T> {
    sender: SyncSender<T>,
    receiver: Mutex<Receiver<T>>,
}

impl<T> GatewayMailbox<T> {
    /// 创建至少容量为 1 的 mailbox。
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        Self {
            sender,
            receiver: Mutex::new(receiver),
        }
    }

    /// 非阻塞写入；满载返回 `false`。
    pub fn try_send(&self, value: T) -> bool {
        match self.sender.try_send(value) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// 在指定时间内读取一条消息。
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<T>, GatewayKernelError> {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match receiver.recv_timeout(timeout) {
            Ok(value) => Ok(Some(value)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(GatewayKernelError::InvalidLifecycle(
                GatewayLifecycle::Stopped,
            )),
        }
    }
}

/// Gateway 内核组合根。
pub struct GatewayKernel {
    state: Arc<AtomicU8>,
    adapters: Mutex<HashMap<String, Arc<dyn GatewayAdapter>>>,
    context: GatewayContext,
}

impl GatewayKernel {
    /// 创建内核；Adapter 共享既有 Daemon ingress/egress。
    #[must_use]
    pub fn new(
        ingress: Arc<dyn GatewayIngress>,
        egress: Arc<dyn GatewayEgress>,
        connection_limit: usize,
    ) -> Self {
        Self::new_with_auth(ingress, egress, connection_limit, None)
    }

    /// 创建使用默认 256 连接上限的内核。
    #[must_use]
    pub fn with_default_connection_limit(
        ingress: Arc<dyn GatewayIngress>,
        egress: Arc<dyn GatewayEgress>,
    ) -> Self {
        Self::new_with_auth(ingress, egress, DEFAULT_CONNECTION_LIMIT, None)
    }

    /// 创建带统一 Gateway token 的内核。
    #[must_use]
    pub fn new_with_auth(
        ingress: Arc<dyn GatewayIngress>,
        egress: Arc<dyn GatewayEgress>,
        connection_limit: usize,
        auth: Option<Arc<GatewayAuth>>,
    ) -> Self {
        let state = Arc::new(AtomicU8::new(STATE_STOPPED));
        Self {
            state: Arc::clone(&state),
            adapters: Mutex::new(HashMap::new()),
            context: GatewayContext::new(
                ingress,
                egress,
                Arc::new(GatewayConnectionLimiter::new(connection_limit)),
                state,
                auth,
            ),
        }
    }

    /// 创建带默认连接上限和统一 Gateway token 的内核。
    #[must_use]
    pub fn with_default_connection_limit_and_auth(
        ingress: Arc<dyn GatewayIngress>,
        egress: Arc<dyn GatewayEgress>,
        auth: Option<Arc<GatewayAuth>>,
    ) -> Self {
        Self::new_with_auth(ingress, egress, DEFAULT_CONNECTION_LIMIT, auth)
    }

    /// 注册 Adapter；运行中和排空状态禁止动态变更，避免竞态。
    pub fn register_adapter(
        &self,
        adapter: Arc<dyn GatewayAdapter>,
    ) -> Result<(), GatewayKernelError> {
        if self.lifecycle() != GatewayLifecycle::Stopped {
            return Err(GatewayKernelError::InvalidLifecycle(self.lifecycle()));
        }
        let adapter_id = adapter.adapter_id().trim().to_owned();
        if adapter_id.is_empty() {
            return Err(GatewayKernelError::EmptyAdapterId);
        }
        let mut adapters = self
            .adapters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if adapters.contains_key(&adapter_id) {
            return Err(GatewayKernelError::DuplicateAdapter(adapter_id));
        }
        adapters.insert(adapter_id, adapter);
        Ok(())
    }

    /// 启动全部 Adapter；任一失败时停止已启动 Adapter 并回到 Stopped。
    pub fn start(&self) -> Result<(), GatewayKernelError> {
        if self
            .state
            .compare_exchange(
                STATE_STOPPED,
                STATE_RUNNING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(GatewayKernelError::InvalidLifecycle(self.lifecycle()));
        }
        let adapters = self.adapter_snapshot();
        let mut started: Vec<Arc<dyn GatewayAdapter>> = Vec::with_capacity(adapters.len());
        for adapter in adapters {
            if let Err(message) = adapter.start(self.context.clone()) {
                for running in started {
                    running.stop();
                }
                self.state.store(STATE_STOPPED, Ordering::Release);
                return Err(GatewayKernelError::Adapter {
                    adapter_id: adapter.adapter_id().to_owned(),
                    message,
                });
            }
            started.push(adapter);
        }
        Ok(())
    }

    /// 开始排空并通知全部 Adapter。
    pub fn begin_drain(&self) -> Result<(), GatewayKernelError> {
        if self
            .state
            .compare_exchange(
                STATE_RUNNING,
                STATE_DRAINING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(GatewayKernelError::InvalidLifecycle(self.lifecycle()));
        }
        for adapter in self.adapter_snapshot() {
            adapter.drain();
        }
        Ok(())
    }

    /// 停止全部 Adapter 并释放 Kernel 状态。
    pub fn stop(&self) -> Result<(), GatewayKernelError> {
        let previous = self.state.swap(STATE_STOPPED, Ordering::AcqRel);
        if previous == STATE_STOPPED {
            return Err(GatewayKernelError::InvalidLifecycle(
                GatewayLifecycle::Stopped,
            ));
        }
        for adapter in self.adapter_snapshot() {
            adapter.stop();
        }
        Ok(())
    }

    /// 当前生命周期。
    #[must_use]
    pub fn lifecycle(&self) -> GatewayLifecycle {
        GatewayLifecycle::from_u8(self.state.load(Ordering::Acquire))
    }

    /// 当前注册 Adapter 数量。
    #[must_use]
    pub fn adapter_count(&self) -> usize {
        self.adapters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 运行状态下取得 Adapter 上下文；排空后拒绝新 ingress。
    pub fn context(&self) -> Result<GatewayContext, GatewayKernelError> {
        match self.lifecycle() {
            GatewayLifecycle::Running => Ok(self.context.clone()),
            state => Err(GatewayKernelError::InvalidLifecycle(state)),
        }
    }

    fn adapter_snapshot(&self) -> Vec<Arc<dyn GatewayAdapter>> {
        self.adapters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }
}

impl std::fmt::Debug for GatewayConnectionPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("GatewayConnectionPermit").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct EmptyIngress;
    impl GatewayIngress for EmptyIngress {
        fn submit_external_event(
            &self,
            _event: ExternalEventEnvelope,
        ) -> Result<SubmitInteractionReceipt, GatewayKernelError> {
            Err(GatewayKernelError::Ingress("测试 ingress".to_owned()))
        }
    }

    struct EmptyEgress;
    impl GatewayEgress for EmptyEgress {
        fn subscribe(
            &self,
            _request: GatewaySubscriptionRequest,
        ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
            Err(GatewayKernelError::Ingress("测试 egress".to_owned()))
        }
    }

    struct TestAdapter {
        id: &'static str,
        starts: AtomicUsize,
        drains: AtomicUsize,
        stops: AtomicUsize,
    }

    impl GatewayAdapter for TestAdapter {
        fn adapter_id(&self) -> &str {
            self.id
        }
        fn start(&self, _context: GatewayContext) -> Result<(), String> {
            self.starts.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn drain(&self) {
            self.drains.fetch_add(1, Ordering::Relaxed);
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn kernel(limit: usize) -> GatewayKernel {
        GatewayKernel::new(Arc::new(EmptyIngress), Arc::new(EmptyEgress), limit)
    }

    #[test]
    fn connection_limit_is_bounded_and_permit_release_reopens_capacity() {
        let limiter = Arc::new(GatewayConnectionLimiter::new(2));
        let first = limiter.try_acquire().expect("首个连接");
        let second = limiter.try_acquire().expect("第二个连接");
        assert_eq!(limiter.active(), 2);
        assert!(matches!(
            limiter.try_acquire(),
            Err(GatewayKernelError::ConnectionLimitReached(2))
        ));
        drop(first);
        assert!(limiter.try_acquire().is_ok());
        drop(second);
    }

    #[test]
    fn default_connection_limit_matches_daemon_ipc_capacity() {
        let limiter = GatewayConnectionLimiter::default_limit();
        let permits = (0..DEFAULT_CONNECTION_LIMIT)
            .map(|_| limiter.try_acquire().expect("默认容量内应可接入"))
            .collect::<Vec<_>>();
        assert_eq!(limiter.active(), DEFAULT_CONNECTION_LIMIT);
        assert!(matches!(
            limiter.try_acquire(),
            Err(GatewayKernelError::ConnectionLimitReached(
                DEFAULT_CONNECTION_LIMIT
            ))
        ));
        drop(permits);
        assert_eq!(limiter.active(), 0);
    }

    #[test]
    fn mailbox_rejects_when_full_without_blocking() {
        let mailbox = GatewayMailbox::new(1);
        assert!(mailbox.try_send(1));
        assert!(!mailbox.try_send(2));
        assert_eq!(mailbox.recv_timeout(Duration::ZERO).expect("读取"), Some(1));
    }

    #[test]
    fn registry_requires_unique_ids_and_drain_rejects_new_context() {
        let gateway = kernel(4);
        let adapter = Arc::new(TestAdapter {
            id: "test",
            starts: AtomicUsize::new(0),
            drains: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
        });
        gateway
            .register_adapter(adapter.clone())
            .expect("注册 Adapter");
        assert_eq!(
            gateway.register_adapter(Arc::new(TestAdapter {
                id: "test",
                starts: AtomicUsize::new(0),
                drains: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
            })),
            Err(GatewayKernelError::DuplicateAdapter("test".to_owned()))
        );
        gateway.start().expect("启动");
        let running_context = gateway.context().expect("运行上下文");
        assert!(running_context.try_acquire_connection().is_ok());
        gateway.begin_drain().expect("排空");
        assert_eq!(adapter.drains.load(Ordering::Relaxed), 1);
        assert!(matches!(
            running_context.try_acquire_connection(),
            Err(GatewayKernelError::InvalidLifecycle(
                GatewayLifecycle::Draining
            ))
        ));
        assert!(matches!(
            gateway.context(),
            Err(GatewayKernelError::InvalidLifecycle(
                GatewayLifecycle::Draining
            ))
        ));
        gateway.stop().expect("停止");
        assert_eq!(adapter.stops.load(Ordering::Relaxed), 1);
    }
}
