//! Action 租约续约契约测试：长于租约的一轮不会被 supervisor 判成 uncertain。

use super::*;
use crate::model::{RuntimeEvent, TurnExecutionError};
use axon_domain::{ConsumerCursor, RuntimeHostId};
use axon_session::{CheckpointTelemetry, ExecutionStore, SessionEventRecord, SqliteRuntimeStore};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// 真实时钟：续约是否生效只能靠墙上时间判定，固定时钟会把租约一直算成「刚领到」。
#[derive(Clone, Copy)]
struct LiveClock;

impl RuntimeClock for LiveClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
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

/// 在轮次中途读两次执行租约：开始时一次、阻塞之后一次。
#[derive(Clone)]
struct LeasePeekExecutor {
    store: SqliteRuntimeStore,
    execution_id: ExecutionId,
    /// 轮次阻塞时长；应显著大于租约时长。
    busy_ms: u64,
    /// 开始时的到期时间与阻塞之后的到期时间。
    observed: Arc<Mutex<(Option<u64>, Option<u64>)>>,
}

impl LeasePeekExecutor {
    fn expiry(&self) -> Option<u64> {
        self.store
            .get_execution(&self.execution_id)
            .ok()
            .flatten()
            .and_then(|execution| execution.lease)
            .map(|lease| lease.expires_at_ms)
    }
}

impl TurnExecutor for LeasePeekExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        let start = self.expiry();
        std::thread::sleep(Duration::from_millis(self.busy_ms));
        let end = self.expiry();
        *self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = (start, end);
        Ok(TurnExecutionOutcome {
            events: vec![RuntimeEvent {
                event_type: "turn_completed".to_owned(),
                payload_json: "{}".to_owned(),
            }],
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

/// 建一个已入队 Action 的 Session；Host lease 以真实时间领取。
fn session_fixture(
    host_id: &str,
    session_id: &str,
    action_id: &str,
) -> (
    axon_test_support::TestTempDir,
    SqliteRuntimeStore,
    DaemonHostRecord,
    DaemonSessionId,
    SessionActionRecord,
) {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let now_ms = LiveClock.now_ms();
    let host = store
        .claim_host(
            &RuntimeHostId::new(host_id).expect("host"),
            1,
            None,
            now_ms,
            86_400_000,
        )
        .expect("claim host");
    let session = DaemonSessionId::new(session_id).expect("session");
    store
        .create_session(&session, "cwd", now_ms)
        .expect("session");
    let action = store
        .enqueue_action(
            &SessionActionRecord {
                action_id: SessionActionId::new(action_id).expect("action"),
                goal_id: None,
                session_id: session.clone(),
                kind: "user_input".to_owned(),
                priority: 0,
                status: SessionActionStatus::Queued,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                goal_revision: None,
                context_version: None,
                claimed_by: None,
                claimed_generation: None,
                lease_expires_at_ms: None,
                attempt: 0,
            },
            now_ms,
        )
        .expect("enqueue action");
    (directory, store, host, session, action)
}

#[test]
fn lease_is_renewed_while_the_turn_runs_longer_than_the_lease() {
    const LEASE_MS: u64 = 300;
    // 轮次时长约为租约的 2.3 倍：中途必然跨过至少一个到期点。
    const BUSY_MS: u64 = 700;
    let (_directory, store, host, session, action) = session_fixture(
        "lease-renewal-host",
        "lease-renewal-session",
        "lease-renewal-action",
    );
    let execution_id = ExecutionId::new(action.action_id.as_str()).expect("id");
    let observed = Arc::new(Mutex::new((None, None)));
    let worker = ExecutionWorker::new(
        store.clone(),
        LeasePeekExecutor {
            store: store.clone(),
            execution_id: execution_id.clone(),
            busy_ms: BUSY_MS,
            observed: Arc::clone(&observed),
        },
        NoopPublisher,
        LiveClock,
        host,
        session,
        LEASE_MS,
    );

    assert_eq!(worker.pump_once().expect("pump"), Some(action.action_id));

    let (start, end) = *observed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (start, end) = (
        start.expect("轮次开始时应有租约"),
        end.expect("轮次结束时应有租约"),
    );
    assert!(
        end > start,
        "执行期未续租：start={start} end={end}（租约 {LEASE_MS}ms，轮次 {BUSY_MS}ms）"
    );
    // 续租之后执行仍按正常路径收口，而不是被隔离成 uncertain。
    let execution = store
        .get_execution(&execution_id)
        .expect("read execution")
        .expect("执行存在");
    assert_ne!(
        execution.status,
        axon_domain::ExecutionStatus::Uncertain,
        "执行不应被判成 uncertain"
    );
}

#[test]
fn renewal_from_a_stale_generation_is_fenced() {
    let (_directory, store, host, session, _action) =
        session_fixture("own-host", "fenced-session", "fenced-action");
    let claimed = store
        .claim_next_action(&session, &host, LiveClock.now_ms(), 300)
        .expect("claim")
        .expect("应领到 Action");
    // 同一 Host 的旧代际不得续租：换掉 generation 即模拟上一代 claim。
    let stale = DaemonHostRecord {
        generation: "stale-generation".to_owned(),
        ..host.clone()
    };
    let error = store
        .renew_action_lease(
            &claimed.action_id,
            &session,
            &stale,
            LiveClock.now_ms(),
            300,
        )
        .expect_err("旧代际续租必须被拒");
    assert!(matches!(error, RuntimeStoreError::Fenced), "{error:?}");
    // 自己的 claim 可以续。
    store
        .renew_action_lease(&claimed.action_id, &session, &host, LiveClock.now_ms(), 300)
        .expect("自己的 claim 应可续租");
}

#[test]
fn zero_lease_window_never_starts_a_renewal_thread() {
    // 租约时长为 0 时不启动续约线程：空闲期零线程、零写入。
    let (_directory, store, host, session, action) =
        session_fixture("zero-lease-host", "zero-lease-session", "zero-lease-action");
    assert!(
        LeaseRenewal::start(store, action.action_id, session, host, LiveClock, 0).is_none(),
        "零租约不应启动续约"
    );
}
