//! Daemon 健康快照 IPC 测试。

use super::*;

use axon_domain::{ConsumerCursor, DeliveryHealth, HealthBacklog, HealthStatusCounts};
use axon_protocol::{DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonRequest};
use axon_session::{CheckpointTelemetry, SessionEventRecord, SqliteRuntimeStore};
use axon_session_runtime::{ActionExecutionRequest, TurnExecutionError, TurnExecutionOutcome};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const TEST_NOW_MS: u64 = 5_000;
const TEST_LEASE_DURATION_MS: u64 = 60_000;
const TEST_PROTOCOL_CLIENT: &str = "daemon-health-test";
const PENDING_DELIVERIES: u64 = 2;
const OLDEST_DELIVERY_AGE_MS: u64 = 1_500;

#[derive(Clone, Copy)]
struct NoopExecutor;

impl TurnExecutor for NoopExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Ok(TurnExecutionOutcome {
            events: Vec::new(),
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

#[derive(Clone, Copy)]
struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        TEST_NOW_MS
    }
}

#[derive(Clone, Copy)]
struct TestProcess;

impl ProcessIdentity for TestProcess {
    fn pid(&self) -> u32 {
        1
    }

    fn process_start_id(&self) -> Option<String> {
        None
    }
}

/// 测试用投递健康探测，返回稳定的结构化指标。
#[derive(Clone, Copy)]
struct FixedDeliveryHealth;

impl crate::ports::DeliveryHealthStore for FixedDeliveryHealth {
    fn read_delivery_health(&self, _now_ms: u64) -> Result<DeliveryHealth, String> {
        Ok(DeliveryHealth {
            counts: HealthStatusCounts::from_statuses([("pending", PENDING_DELIVERIES)]),
            backlog: HealthBacklog {
                pending: PENDING_DELIVERIES,
                oldest_age_ms: Some(OLDEST_DELIVERY_AGE_MS),
            },
        })
    }
}

/// 可推进时钟：用于验证 lease 续租后的健康指标不依赖启动瞬间的时间。
#[derive(Clone)]
struct AdvancingClock(Arc<AtomicU64>);

impl AdvancingClock {
    fn new(now_ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(now_ms)))
    }

    fn set(&self, now_ms: u64) {
        self.0.store(now_ms, Ordering::Release);
    }
}

impl RuntimeClock for AdvancingClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

fn test_host(
    store: SqliteRuntimeStore,
) -> DaemonHost<SqliteRuntimeStore, NoopExecutor, NoopPublisher, FixedClock, TestProcess> {
    DaemonHost::new(
        store,
        NoopExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
        RuntimeHostId::new("daemon-health-host").expect("Host ID"),
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    )
}

fn request(request_id: &str, command: DaemonCommand) -> DaemonRequest {
    DaemonRequest {
        message_type: "command".to_owned(),
        request_id: request_id.to_owned(),
        protocol_version: DAEMON_PROTOCOL_VERSION,
        client_id: TEST_PROTOCOL_CLIENT.to_owned(),
        session_id: None,
        command,
    }
}

#[test]
fn ipc_get_health_reports_runtime_and_delivery_metrics_without_session_id() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut host = test_host(store).with_delivery_health_store(FixedDeliveryHealth);
    host.start().expect("启动 Host");

    let responses = host.handle_request(request("health", DaemonCommand::GetHealth));

    assert_eq!(responses.len(), 1);
    assert!(responses[0].ok, "健康查询必须成功：{:?}", responses[0].body);
    let body = &responses[0].body;
    assert!(
        body["host"]["generation"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(body["host"]["status"], "running");
    assert!(body["interactions"]["total"].is_u64());
    assert!(body["executions"]["total"].is_u64());
    assert!(body["executionBacklog"]["pending"].is_u64());
    assert_eq!(body["deliveries"]["backlog"]["pending"], PENDING_DELIVERIES);
    assert_eq!(
        body["deliveries"]["backlog"]["oldestAgeMs"],
        OLDEST_DELIVERY_AGE_MS
    );
    assert_eq!(
        body["deliveries"]["counts"]["byStatus"]["pending"],
        PENDING_DELIVERIES
    );
}

#[test]
fn ipc_get_health_without_delivery_probe_reports_runtime_only() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut host = test_host(store);
    host.start().expect("启动 Host");

    let responses = host.handle_request(request("health", DaemonCommand::GetHealth));

    assert!(responses[0].ok);
    assert!(responses[0].body["deliveries"].is_null());
    assert_eq!(responses[0].body["goals"]["total"], 0);
}

#[test]
fn ipc_get_health_reports_renewed_lease_remaining_instead_of_stale_claim() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let clock = AdvancingClock::new(TEST_NOW_MS);
    let mut host = DaemonHost::new(
        store,
        NoopExecutor,
        NoopPublisher,
        clock.clone(),
        TestProcess,
        RuntimeHostId::new("daemon-health-host").expect("Host ID"),
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    );
    host.start().expect("启动 Host");
    // 前进半程后仍未过期，续租必须把剩余租期推回完整窗口。
    clock.set(TEST_NOW_MS + TEST_LEASE_DURATION_MS / 2);
    host.renew().expect("续租 Host lease");

    let snapshot = host.health_snapshot().expect("健康快照");

    assert_eq!(
        snapshot.host.lease_remaining_ms, TEST_LEASE_DURATION_MS,
        "健康快照必须报告续租后的真实剩余租期"
    );
    assert_eq!(
        snapshot.host.lease_expires_at_ms,
        TEST_NOW_MS + TEST_LEASE_DURATION_MS + TEST_LEASE_DURATION_MS / 2
    );
}

#[test]
fn drain_state_is_shared_with_worker_clones_and_blocks_new_work() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut host = test_host(store);
    host.start().expect("启动 Host");
    let worker_view = host.clone();

    host.begin_drain().expect("进入排空");

    let health = worker_view.health_snapshot().expect("健康快照");
    assert_eq!(health.host.status, "admitting_shutdown");
    assert!(matches!(
        worker_view.primary_turn_tick(1),
        Ok(report) if report.claimed == 0
    ));
    assert!(matches!(
        worker_view.scheduler_tick(TEST_LEASE_DURATION_MS, 1),
        Ok(report) if report.claimed == 0
    ));
}
