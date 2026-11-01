//! Gateway 内核的生命周期、连接准入与 mailbox 契约测试。
//!
//! 与 `gateway_kernel.rs` 分文件存放：内核实现本身已接近单文件行数硬限，测试与实现
//! 按职责分开，新增用例不会再挤占实现的篇幅。

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
