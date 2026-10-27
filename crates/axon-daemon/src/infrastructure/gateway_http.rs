//! Gateway HTTP Webhook 传输适配器。
//!
//! 传输由 axum（hyper + tokio）承载：请求解析、连接管理、优雅停机都交给框架，
//! 本模块只负责生命周期与运行态装配。同步业务统一在线程池执行，运行时的 IO
//! 线程不再被慢请求占住，因此单个慢请求不会把其后所有请求一起拖住。
//!
//! 生命周期口径未变：`start` 同步完成监听绑定，绑定失败如实上抛；`drain` 只
//! 发出停机信号，不阻塞调用方；`stop` 发出信号并等待服务线程收口。

#[path = "gateway_http_events.rs"]
mod gateway_http_events;
#[path = "gateway_http_response.rs"]
mod gateway_http_response;
#[path = "gateway_http_routes.rs"]
mod gateway_http_routes;
#[path = "gateway_voice.rs"]
mod gateway_voice;
#[path = "gateway_voice_tts.rs"]
mod gateway_voice_tts;

pub use super::gateway_http_handler::{GatewayHttpResponse, GatewayHttpRouteHandler};

use super::event_scripts::EventProviderRegistry;
use super::gateway_kernel::{GatewayAdapter, GatewayContext};
use super::log_sink::LogSink;
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::{Semaphore, watch};

/// Adapter ID。
const ADAPTER_ID: &str = "gateway-http";
/// 请求体默认上限。
///
/// 传输层不靠限制解决问题：工具结果等大内容在进入事件流之前就按预算裁剪并落归档，
/// 因此这里的上限只用来挡住失控的对端，取值与 dsh 的 HTTP 面一致（300 MiB）。
/// 超过时按 413 明确拒绝，不做静默截断。
const DEFAULT_MAX_BODY_BYTES: usize = 300 * 1024 * 1024;
/// 运行时工作线程数：只承担协议与调度，业务在阻塞池执行。
const RUNTIME_WORKER_THREADS: usize = 4;
/// 阻塞型业务的并发上限；与既有实现的 worker × 队列量级对齐。
const BLOCKING_CONCURRENCY: usize = 64;
/// 阻塞池排队预算：超过即快速失败，不无限排队。
const SATURATION_WAIT: Duration = Duration::from_secs(5);
/// 慢请求阈值：超过即按 WARNING 记录，供事后归因。
const SLOW_REQUEST_MS: u128 = 500;
/// 请求头总字节上限。
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// 网关运行态：请求处理共享的一份状态。
pub(super) struct GatewayHttpState {
    /// 内核上下文：事件提交、交互查询、鉴权与连接准入都经它。
    pub(super) context: GatewayContext,
    /// provider 事件注册表。
    pub(super) providers: Arc<EventProviderRegistry>,
    /// 外部扩展路由处理器。
    pub(super) route_handler: Option<Arc<dyn GatewayHttpRouteHandler>>,
    /// 请求体上限。
    pub(super) max_body_bytes: usize,
    /// 结构化日志出口；未注入时静默，不影响请求路径。
    log_sink: Option<Arc<dyn LogSink>>,
    /// 阻塞型业务的并发闸门。
    pub(super) blocking: Arc<Semaphore>,
}

impl GatewayHttpState {
    /// 写一条访问日志；未注入 sink 时什么也不做。
    pub(super) fn log_access(&self, level: &str, message: &str) {
        if let Some(sink) = self.log_sink.as_deref() {
            sink.append(level, message);
        }
    }
}

/// HTTP Webhook 适配器。
pub struct HttpGatewayAdapter {
    bind_addr: String,
    max_body_bytes: usize,
    providers: Arc<EventProviderRegistry>,
    shutdown: Mutex<Option<watch::Sender<bool>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    local_addr: Mutex<Option<SocketAddr>>,
    route_handler: Option<Arc<dyn GatewayHttpRouteHandler>>,
    log_sink: Option<Arc<dyn LogSink>>,
}

impl HttpGatewayAdapter {
    /// 创建使用默认 body 上限的 HTTP 适配器。
    #[must_use]
    pub fn new(bind_addr: impl Into<String>, providers: Arc<EventProviderRegistry>) -> Self {
        Self::new_with_limit(bind_addr, DEFAULT_MAX_BODY_BYTES, providers)
    }

    /// 创建指定 body 上限的 HTTP 适配器。
    #[must_use]
    pub fn new_with_limit(
        bind_addr: impl Into<String>,
        max_body_bytes: usize,
        providers: Arc<EventProviderRegistry>,
    ) -> Self {
        Self {
            bind_addr: bind_addr.into(),
            max_body_bytes: max_body_bytes.max(1),
            providers,
            shutdown: Mutex::new(None),
            handle: Mutex::new(None),
            local_addr: Mutex::new(None),
            route_handler: None,
            log_sink: None,
        }
    }

    /// 注册外部扩展路由处理器（例如可观测性 RESTful API）。
    #[must_use]
    pub fn with_route_handler(mut self, handler: Arc<dyn GatewayHttpRouteHandler>) -> Self {
        self.route_handler = Some(handler);
        self
    }

    /// 注入结构化日志出口；访问日志与饱和告警都写在这里。
    #[must_use]
    pub fn with_log_sink(mut self, sink: Arc<dyn LogSink>) -> Self {
        self.log_sink = Some(sink);
        self
    }

    /// 实际监听地址；未启动时为 None。绑定端口写 0 时用它取回真实端口。
    #[must_use]
    pub fn local_addr(&self) -> Option<SocketAddr> {
        *self
            .local_addr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 发出停机信号：`drain` 与 `stop` 共用，重复发出是幂等的。
    fn signal_stop(&self) {
        let guard = self
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sender) = guard.as_ref() {
            let _ = sender.send(true);
        }
    }
}

impl GatewayAdapter for HttpGatewayAdapter {
    fn adapter_id(&self) -> &str {
        ADAPTER_ID
    }

    fn start(&self, context: GatewayContext) -> Result<(), String> {
        let listener = TcpListener::bind(&self.bind_addr)
            .map_err(|error| format!("HTTP Gateway 监听 {} 失败：{error}", self.bind_addr))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("HTTP Gateway 设置非阻塞 listener 失败：{error}"))?;
        let bound = listener
            .local_addr()
            .map_err(|error| format!("HTTP Gateway 读取监听地址失败：{error}"))?;

        let (shutdown, receiver) = watch::channel(false);
        *self
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(shutdown);
        *self
            .local_addr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bound);

        let state = Arc::new(GatewayHttpState {
            context,
            providers: Arc::clone(&self.providers),
            route_handler: self.route_handler.clone(),
            max_body_bytes: self.max_body_bytes,
            log_sink: self.log_sink.clone(),
            blocking: Arc::new(Semaphore::new(BLOCKING_CONCURRENCY)),
        });
        let handle = thread::spawn(move || run_server(listener, receiver, state));
        *self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle);
        Ok(())
    }

    fn drain(&self) {
        self.signal_stop();
    }

    fn stop(&self) {
        self.signal_stop();
        if let Some(handle) = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = handle.join();
        }
    }
}

/// 服务线程：建运行时、接管监听、优雅停机。
fn run_server(
    listener: TcpListener,
    shutdown: watch::Receiver<bool>,
    state: Arc<GatewayHttpState>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RUNTIME_WORKER_THREADS)
        .enable_all()
        .thread_name("axon-gateway-http")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            state.log_access("ERROR", &format!("gateway-http 运行时创建失败：{error}"));
            return;
        }
    };
    let max_body_bytes = state.max_body_bytes;
    let failure_state = Arc::clone(&state);
    let outcome = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)
            .map_err(|error| format!("接管监听失败：{error}"))?;
        let router = gateway_http_routes::router(state, max_body_bytes);
        axum::serve(listener, router)
            .with_graceful_shutdown(wait_for_shutdown(shutdown))
            .await
            .map_err(|error| format!("服务退出：{error}"))
    });
    if let Err(message) = outcome {
        failure_state.log_access("ERROR", &format!("gateway-http {message}"));
    }
}

/// 停机信号：等到 `drain`/`stop` 发出的一次信号，通道关闭时同样收口。
async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    while !*shutdown.borrow_and_update() {
        if shutdown.changed().await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
#[path = "gateway_http_tests.rs"]
mod gateway_http_tests;
