use super::*;
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionSource,
    InteractionStatus, ReplyRoute, ScopeRef,
};
use axon_session::{InteractionAppendReceipt, InteractionProcessingLease};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

const TEST_NOW_MS: u64 = 100;
const TEST_LEASE_DURATION_MS: u64 = 1_000;
const TEST_LEASE_TOKEN: &str = "lease-1";
const RENEWAL_TEST_LEASE_DURATION_MS: u64 = 40;
const RENEWAL_WAIT_POLLS: usize = 200;
const RENEWAL_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(5);
/// 并发 session 汇合点的参与方数量：workspace 与会话表面相同的 session 汇为一组。
const SESSION_RENDEZVOUS_PARTIES: usize = 2;
/// 汇合点等待上限；健康实现立即汇合，回归为串行时由断言而非挂起收口。
const SESSION_RENDEZVOUS_TIMEOUT: Duration = Duration::from_secs(5);
/// 慢 session 的 Interaction：占用 1 个 session 槽位做长 turn。
const SLOW_SESSION_INTERACTION_ID: &str = "slow-1";
/// 长 turn 执行期间追加到空闲 session 的后续消息。
const IDLE_SESSION_FOLLOW_UP_ID: &str = "b-2";
/// 空闲 session 后续消息被独立领取执行的等待上限。
const IDLE_SESSION_PROGRESS_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
struct TestClock;

impl RuntimeClock for TestClock {
    fn now_ms(&self) -> u64 {
        TEST_NOW_MS
    }
}

#[derive(Clone, Default)]
struct TestStore {
    state: Arc<Mutex<Option<(InteractionRecord, String)>>>,
    renewals: Arc<AtomicUsize>,
}

impl TestStore {
    fn transition(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        status: InteractionStatus,
    ) -> Result<(), InteractionStoreError> {
        let mut state = self.state.lock().expect("锁未中毒");
        let Some((record, token)) = state.as_mut() else {
            return Err(InteractionStoreError::NotFound(interaction_id.clone()));
        };
        if &record.interaction_id != interaction_id
            || record.status != InteractionStatus::Processing
            || token != lease_token
        {
            return Err(InteractionStoreError::InvalidState(
                "lease 不匹配".to_owned(),
            ));
        }
        record.status = status;
        token.clear();
        Ok(())
    }
}

impl InteractionStore for TestStore {
    fn append(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<InteractionAppendReceipt, InteractionStoreError> {
        *self.state.lock().expect("锁未中毒") = Some((interaction.clone(), String::new()));
        Ok(InteractionAppendReceipt {
            interaction_id: interaction.interaction_id.clone(),
            inserted: true,
        })
    }

    fn claim_processing(
        &self,
        _limit: u32,
        _now_ms: u64,
        _lease_duration_ms: u64,
    ) -> Result<Vec<InteractionProcessingLease>, InteractionStoreError> {
        let mut state = self.state.lock().expect("锁未中毒");
        let Some((record, token)) = state.as_mut() else {
            return Ok(Vec::new());
        };
        if record.status != InteractionStatus::Received {
            return Ok(Vec::new());
        }
        record.status = InteractionStatus::Processing;
        *token = TEST_LEASE_TOKEN.to_owned();
        Ok(vec![InteractionProcessingLease {
            interaction: record.clone(),
            lease_token: token.clone(),
            lease_expires_at_ms: TEST_LEASE_DURATION_MS,
        }])
    }

    fn claim_processing_by_id(
        &self,
        interaction_id: &InteractionId,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Option<InteractionProcessingLease>, InteractionStoreError> {
        let matches = self
            .state
            .lock()
            .expect("锁未中毒")
            .as_ref()
            .is_some_and(|(record, _)| &record.interaction_id == interaction_id);
        if !matches {
            return Ok(None);
        }
        Ok(self
            .claim_processing(1, now_ms, lease_duration_ms)?
            .into_iter()
            .next())
    }

    fn renew_processing(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<u64, InteractionStoreError> {
        self.renewals.fetch_add(1, Ordering::SeqCst);
        let state = self.state.lock().expect("锁未中毒");
        let Some((record, token)) = state.as_ref() else {
            return Err(InteractionStoreError::NotFound(interaction_id.clone()));
        };
        if &record.interaction_id != interaction_id
            || record.status != InteractionStatus::Processing
            || token != lease_token
        {
            return Err(InteractionStoreError::InvalidState(
                "lease 不匹配".to_owned(),
            ));
        }
        Ok(now_ms.saturating_add(lease_duration_ms))
    }

    fn release_processing(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        self.transition(interaction_id, lease_token, InteractionStatus::Received)
    }

    fn mark_understood(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        self.transition(interaction_id, lease_token, InteractionStatus::Understood)
    }

    fn mark_rejected(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        self.transition(interaction_id, lease_token, InteractionStatus::Rejected)
    }

    fn mark_interrupted(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        self.transition(interaction_id, lease_token, InteractionStatus::Interrupted)
    }
}

#[derive(Default)]
struct BatchState {
    pending: Vec<InteractionRecord>,
    active: Vec<InteractionProcessingLease>,
}

#[derive(Clone, Default)]
struct BatchStore {
    state: Arc<Mutex<BatchState>>,
}

impl BatchStore {
    fn settle(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
    ) -> Result<(), InteractionStoreError> {
        let mut state = self.state.lock().expect("锁未中毒");
        let Some(index) = active_lease_index(&state.active, interaction_id, lease_token) else {
            return Err(InteractionStoreError::InvalidState(
                "lease 不匹配".to_owned(),
            ));
        };
        state.active.remove(index);
        Ok(())
    }
}

impl InteractionStore for BatchStore {
    fn append(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<InteractionAppendReceipt, InteractionStoreError> {
        self.state
            .lock()
            .expect("锁未中毒")
            .pending
            .push(interaction.clone());
        Ok(InteractionAppendReceipt {
            interaction_id: interaction.interaction_id.clone(),
            inserted: true,
        })
    }

    fn claim_processing(
        &self,
        _limit: u32,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Vec<InteractionProcessingLease>, InteractionStoreError> {
        let mut state = self.state.lock().expect("锁未中毒");
        let records = std::mem::take(&mut state.pending);
        let leases = records
            .into_iter()
            .map(|mut interaction| {
                interaction.status = InteractionStatus::Processing;
                InteractionProcessingLease {
                    lease_token: format!("lease-{}", interaction.interaction_id.as_str()),
                    interaction,
                    lease_expires_at_ms: now_ms.saturating_add(lease_duration_ms),
                }
            })
            .collect::<Vec<_>>();
        state.active.extend(leases.iter().cloned());
        Ok(leases)
    }

    fn claim_processing_by_id(
        &self,
        interaction_id: &InteractionId,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Option<InteractionProcessingLease>, InteractionStoreError> {
        let mut state = self.state.lock().expect("锁未中毒");
        let Some(index) = state
            .pending
            .iter()
            .position(|record| &record.interaction_id == interaction_id)
        else {
            return Ok(None);
        };
        let mut interaction = state.pending.remove(index);
        interaction.status = InteractionStatus::Processing;
        let lease = InteractionProcessingLease {
            lease_token: format!("lease-{}", interaction.interaction_id.as_str()),
            interaction,
            lease_expires_at_ms: now_ms.saturating_add(lease_duration_ms),
        };
        state.active.push(lease.clone());
        Ok(Some(lease))
    }

    fn renew_processing(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<u64, InteractionStoreError> {
        let mut state = self.state.lock().expect("锁未中毒");
        let Some(index) = active_lease_index(&state.active, interaction_id, lease_token) else {
            return Err(InteractionStoreError::InvalidState(
                "lease 不匹配".to_owned(),
            ));
        };
        let expires_at_ms = now_ms.saturating_add(lease_duration_ms);
        state.active[index].lease_expires_at_ms = expires_at_ms;
        Ok(expires_at_ms)
    }

    fn release_processing(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        let mut state = self.state.lock().expect("锁未中毒");
        let Some(index) = active_lease_index(&state.active, interaction_id, lease_token) else {
            return Err(InteractionStoreError::InvalidState(
                "lease 不匹配".to_owned(),
            ));
        };
        let mut interaction = state.active.remove(index).interaction;
        interaction.status = InteractionStatus::Received;
        state.pending.push(interaction);
        Ok(())
    }

    fn mark_understood(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        self.settle(interaction_id, lease_token)
    }

    fn mark_rejected(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        self.settle(interaction_id, lease_token)
    }

    fn mark_interrupted(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        self.settle(interaction_id, lease_token)
    }
}

fn active_lease_index(
    active: &[InteractionProcessingLease],
    interaction_id: &InteractionId,
    lease_token: &str,
) -> Option<usize> {
    active.iter().position(|lease| {
        &lease.interaction.interaction_id == interaction_id && lease.lease_token == lease_token
    })
}

#[derive(Clone, Default)]
struct RecordingRunner {
    calls: Arc<Mutex<Vec<String>>>,
    profiles: Arc<Mutex<Vec<RuntimeProfile>>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    /// 可选汇合点：需要观察 session 并发时注入，用同步取代 sleep 时序。
    rendezvous: Option<Arc<SessionRendezvous>>,
}

/// 两个 session 的确定性汇合点。
struct SessionRendezvous {
    arrived: Mutex<usize>,
    ready: Condvar,
}

impl SessionRendezvous {
    fn new() -> Self {
        Self {
            arrived: Mutex::new(0),
            ready: Condvar::new(),
        }
    }

    /// 登记本轮进入并等待另一个 session；超时后返回，让并发断言给出确定性失败。
    fn meet(&self) {
        let mut arrived = self.arrived.lock().expect("锁未中毒");
        *arrived += 1;
        self.ready.notify_all();
        if *arrived >= SESSION_RENDEZVOUS_PARTIES {
            return;
        }
        let _ = self
            .ready
            .wait_timeout_while(arrived, SESSION_RENDEZVOUS_TIMEOUT, |count| {
                *count < SESSION_RENDEZVOUS_PARTIES
            })
            .expect("锁未中毒");
    }
}

impl PrimaryTurnRunner for RecordingRunner {
    fn run(&self, request: PrimaryTurnRequest) -> Result<(), PrimaryTurnRunnerError> {
        self.calls
            .lock()
            .expect("锁未中毒")
            .push(request.interaction.interaction_id.as_str().to_owned());
        self.profiles
            .lock()
            .expect("锁未中毒")
            .push(request.runtime_profile);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        if let Some(rendezvous) = &self.rendezvous {
            rendezvous.meet();
        }
        std::thread::sleep(Duration::from_millis(1));
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Clone, Default)]
struct FailingRunner;

impl PrimaryTurnRunner for FailingRunner {
    fn run(&self, _request: PrimaryTurnRequest) -> Result<(), PrimaryTurnRunnerError> {
        Err(PrimaryTurnRunnerError::Provider(
            "输入不满足安全策略".to_owned(),
        ))
    }
}

#[derive(Clone, Default)]
struct WaitForLeaseRenewalRunner {
    renewals: Arc<AtomicUsize>,
}

impl PrimaryTurnRunner for WaitForLeaseRenewalRunner {
    fn run(&self, _request: PrimaryTurnRequest) -> Result<(), PrimaryTurnRunnerError> {
        for _ in 0..RENEWAL_WAIT_POLLS {
            if self.renewals.load(Ordering::SeqCst) > 0 {
                return Ok(());
            }
            std::thread::sleep(RENEWAL_WAIT_POLL_INTERVAL);
        }
        Err(PrimaryTurnRunnerError::Runtime(
            "Primary turn lease 未续租".to_owned(),
        ))
    }
}

fn interaction() -> InteractionRecord {
    InteractionRecord::new(
        axon_domain::DaemonSessionId::new("01J8XQ7K3M4N5P6Q7R8S9T0V1W").expect("合法会话 ID"),
        InteractionId::new("interaction-primary").expect("有效 ID"),
        InteractionSource::new("tui", "message-primary").expect("有效来源"),
        ScopeRef::workspace("workspace-primary").expect("有效作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", "terminal-primary").expect("有效路由"),
        "检查状态",
        1,
    )
    .expect("Interaction 有效")
}

fn interaction_in_workspace(id: &str, workspace: &str, received_at_ms: u64) -> InteractionRecord {
    InteractionRecord::new(
        // 测试夹具：会话标识是显式输入，这里直接用 Workspace 名，不经过任何派生逻辑。
        axon_domain::DaemonSessionId::new(workspace).expect("合法会话 ID"),
        InteractionId::new(id).expect("有效 ID"),
        InteractionSource::new("tui", format!("message-{id}")).expect("有效来源"),
        ScopeRef::workspace(workspace).expect("有效作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", "terminal-shared").expect("有效路由"),
        id,
        received_at_ms,
    )
    .expect("Interaction 有效")
}

#[test]
fn primary_turn_uses_axon_profile_regardless_of_transport_source() {
    let mut source_interaction = interaction();
    source_interaction.source =
        InteractionSource::new("webhook", "message-from-remote").expect("有效来源");
    let store = TestStore::default();
    store.append(&source_interaction).expect("追加成功");
    let runner = RecordingRunner::default();
    let service = PrimaryTurnService::new(store, runner.clone(), TestClock, TEST_LEASE_DURATION_MS)
        .expect("服务创建成功");

    service.tick(1).expect("主 turn 成功");

    assert_eq!(
        runner.profiles.lock().expect("锁未中毒").as_slice(),
        [RuntimeProfile::Primary]
    );
}

#[test]
fn concurrent_tick_keeps_session_order_and_bounds_session_count() {
    let store = BatchStore::default();
    for (id, workspace, received_at_ms) in [
        ("a-1", "workspace-a", 1),
        ("a-2", "workspace-a", 2),
        ("b-1", "workspace-b", 1),
    ] {
        store
            .append(&interaction_in_workspace(id, workspace, received_at_ms))
            .expect("追加成功");
    }
    let runner = RecordingRunner {
        rendezvous: Some(Arc::new(SessionRendezvous::new())),
        ..RecordingRunner::default()
    };
    let service = PrimaryTurnService::new(store, runner.clone(), TestClock, TEST_LEASE_DURATION_MS)
        .expect("服务创建成功");

    let report = service.tick_concurrent(10, 2).expect("并发 tick 成功");

    assert_eq!(report.claimed, 3);
    assert_eq!(report.understood, 3);
    assert!(runner.max_active.load(Ordering::SeqCst) <= 2);
    assert_eq!(runner.max_active.load(Ordering::SeqCst), 2);
    let calls = runner.calls.lock().expect("锁未中毒");
    let workspace_a_calls = calls
        .iter()
        .filter(|id| id.starts_with("a-"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(workspace_a_calls, ["a-1", "a-2"]);
}

/// 单向信号：布尔标志 + 条件变量，用于替代 sleep 时序。
type Signal = Arc<(Mutex<bool>, Condvar)>;

fn new_signal() -> Signal {
    Arc::new((Mutex::new(false), Condvar::new()))
}

fn signal_once(signal: &Signal) {
    let (signaled, condvar) = &**signal;
    *signaled.lock().expect("锁未中毒") = true;
    condvar.notify_all();
}

fn wait_for_signal(signal: &Signal, timeout: Duration) -> bool {
    let (signaled, condvar) = &**signal;
    let signaled = signaled.lock().expect("锁未中毒");
    let (signaled, _) = condvar
        .wait_timeout_while(signaled, timeout, |signaled| !*signaled)
        .expect("锁未中毒");
    *signaled
}

/// 慢 session 的长 turn 会向空闲 session 追加新消息，并等待它被独立领取执行。
///
/// 回归 T-4：批间 barrier 下这条消息必须等长 turn 结束才会被领取。
#[derive(Clone)]
struct IdleSessionProgressRunner {
    store: BatchStore,
    long_turn_entered: Signal,
    idle_session_entered: Signal,
    release: Signal,
}

impl PrimaryTurnRunner for IdleSessionProgressRunner {
    fn run(&self, request: PrimaryTurnRequest) -> Result<(), PrimaryTurnRunnerError> {
        match request.interaction.interaction_id.as_str() {
            SLOW_SESSION_INTERACTION_ID => {
                signal_once(&self.long_turn_entered);
                self.store
                    .append(&interaction_in_workspace(
                        IDLE_SESSION_FOLLOW_UP_ID,
                        "workspace-b",
                        3,
                    ))
                    .map_err(|error| PrimaryTurnRunnerError::Runtime(error.to_string()))?;
                if !wait_for_signal(&self.release, IDLE_SESSION_PROGRESS_TIMEOUT) {
                    return Err(PrimaryTurnRunnerError::Runtime(
                        "长 turn 未被放行".to_owned(),
                    ));
                }
            }
            IDLE_SESSION_FOLLOW_UP_ID => signal_once(&self.idle_session_entered),
            _ => {}
        }
        Ok(())
    }
}

#[test]
fn idle_session_progresses_without_waiting_for_the_slowest_session() {
    let store = BatchStore::default();
    store
        .append(&interaction_in_workspace(
            SLOW_SESSION_INTERACTION_ID,
            "workspace-a",
            1,
        ))
        .expect("追加慢 session 消息");
    store
        .append(&interaction_in_workspace("b-1", "workspace-b", 2))
        .expect("追加空闲 session 已有消息");
    let runner = IdleSessionProgressRunner {
        store: store.clone(),
        long_turn_entered: new_signal(),
        idle_session_entered: new_signal(),
        release: new_signal(),
    };
    let service = PrimaryTurnService::new(store, runner.clone(), TestClock, TEST_LEASE_DURATION_MS)
        .expect("服务创建成功");
    let tick = thread::spawn(move || service.tick_concurrent(10, 4));

    assert!(
        wait_for_signal(&runner.long_turn_entered, IDLE_SESSION_PROGRESS_TIMEOUT),
        "慢 session 必须在本批内开始执行"
    );
    // 长 turn 正在运行时它追加的空闲 session 消息必须被补位领取，而不是等长 turn 收口。
    let idle_session_progressed =
        wait_for_signal(&runner.idle_session_entered, IDLE_SESSION_PROGRESS_TIMEOUT);
    signal_once(&runner.release);

    let report = tick
        .join()
        .expect("Primary Turn 协调线程不得 panic")
        .expect("并发 tick 成功");
    assert!(
        idle_session_progressed,
        "空闲 session 的新消息不得等待最慢 session 结束才启动"
    );
    assert_eq!(report.claimed, 3);
    assert_eq!(report.understood, 3);
}

#[path = "primary_turn_service_lease_tests.rs"]
mod primary_turn_service_lease_tests;
