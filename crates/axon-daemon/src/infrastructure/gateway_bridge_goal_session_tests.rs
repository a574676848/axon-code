//! 会话订阅放行契约测试：会话存在性与受众边界一律读 `sessions` 实列，不解析标识前缀。

use super::*;
use crate::application::{EventStreamBridge, NoActiveTurnSteerSink};
use axon_domain::DaemonSessionId;
use axon_session::{
    NewSessionRequest, RuntimeStore, SessionKind, SessionTrustScope, SqliteRuntimeStore,
};

const ACTION_LEASE_DURATION_MS: u64 = 120_000;
const NOW_MS: u64 = 1_760_000_000_000;
const OTHER_WORKSPACE: &str = "workspace-other";

/// 固定时钟：判定与 lease 都不依赖真实时间。
#[derive(Clone)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}

struct Fixture {
    store: SqliteRuntimeStore,
    stream_bridge: Arc<EventStreamBridge>,
    // 字段顺序即析构顺序：临时目录最后析构，先释放 SQLite 句柄。
    _temp: axon_test_support::TestTempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = axon_test_support::test_temp_dir();
        let data_root = temp.path().join("data");
        let store = SqliteRuntimeStore::from_root(&data_root);
        store.initialize().expect("初始化运行态");
        Self {
            store,
            stream_bridge: Arc::new(EventStreamBridge::new()),
            _temp: temp,
        }
    }

    fn bridge(&self) -> DaemonGatewayBridge<SqliteRuntimeStore, FixedClock> {
        DaemonGatewayBridge::new(GatewayHostDependencies {
            store: self.store.clone(),
            active_turn_steer_sink: Arc::new(NoActiveTurnSteerSink),
            clock: FixedClock,
            action_lease_duration_ms: ACTION_LEASE_DURATION_MS,
            stream_bridge: Arc::clone(&self.stream_bridge),
            host_generation: None,
        })
    }

    /// 登记一条会话：类型、归属与受众边界都是显式输入。
    fn session(
        &self,
        session_id: &str,
        workspace_id: Option<&str>,
        trust_scope: SessionTrustScope,
    ) -> DaemonSessionId {
        let session_id = DaemonSessionId::new(session_id).expect("session id");
        self.store
            .create_session_with(&NewSessionRequest {
                session_id: session_id.clone(),
                kind: SessionKind::Primary,
                trust_scope,
                workspace_id: workspace_id.map(str::to_owned),
                root_session_id: None,
                agent_path: None,
                goal_id: None,
                execution_id: None,
                cwd: "/repo-a".to_owned(),
                now_ms: NOW_MS,
            })
            .expect("登记会话");
        session_id
    }

    fn subscribe(
        &self,
        workspace_id: &str,
        session_id: &DaemonSessionId,
    ) -> Result<(), GatewayKernelError> {
        let bridge = self.bridge();
        GatewayEgress::subscribe(
            &bridge,
            GatewaySubscriptionRequest {
                workspace_id: workspace_id.to_owned(),
                interaction_id: String::new(),
                session_id: Some(session_id.as_str().to_owned()),
                resume_cursor: None,
            },
        )
        .map(|_| ())
    }
}

#[test]
fn session_in_the_requested_workspace_is_allowed() {
    let fixture = Fixture::new();
    let session_id = fixture.session("session-a", Some("workspace-a"), SessionTrustScope::Private);
    assert!(fixture.subscribe("workspace-a", &session_id).is_ok());
}

#[test]
fn session_bound_to_another_workspace_is_rejected() {
    let fixture = Fixture::new();
    let session_id = fixture.session("session-a", Some("workspace-a"), SessionTrustScope::Private);
    // 会话存在，但请求工作区不符：必须拒绝。
    let error = fixture
        .subscribe(OTHER_WORKSPACE, &session_id)
        .expect_err("工作区不符应拒绝");
    assert!(
        matches!(error, GatewayKernelError::Ingress(ref reason) if reason.contains("工作区不匹配")),
        "{error:?}"
    );
}

#[test]
fn unregistered_session_is_rejected() {
    let fixture = Fixture::new();
    let session_id = DaemonSessionId::new("session-never-registered").expect("session id");
    let error = fixture
        .subscribe("workspace-a", &session_id)
        .expect_err("未登记会话应拒绝");
    assert!(matches!(error, GatewayKernelError::Ingress(_)), "{error:?}");
}

#[test]
fn restricted_trust_scope_is_rejected_without_audience_proof() {
    let fixture = Fixture::new();
    let session_id = fixture.session(
        "session-restricted",
        Some("workspace-a"),
        SessionTrustScope::Restricted,
    );
    let error = fixture
        .subscribe("workspace-a", &session_id)
        .expect_err("受限受众应拒绝");
    assert!(
        matches!(error, GatewayKernelError::Ingress(ref reason) if reason.contains("受众边界")),
        "{error:?}"
    );
}

#[test]
fn group_workspace_scope_is_allowed_for_the_same_workspace() {
    let fixture = Fixture::new();
    let session_id = fixture.session(
        "session-group",
        Some("workspace-a"),
        SessionTrustScope::GroupWorkspace,
    );
    assert!(fixture.subscribe("workspace-a", &session_id).is_ok());
}

#[test]
fn session_without_workspace_binding_is_not_restricted_by_workspace() {
    let fixture = Fixture::new();
    let session_id = fixture.session("session-global", None, SessionTrustScope::Private);
    assert!(fixture.subscribe("workspace-a", &session_id).is_ok());
    assert!(fixture.subscribe(OTHER_WORKSPACE, &session_id).is_ok());
}

/// W9.2：同一会话的重复放行判定由缓存承担，不再每次回表。
///
/// 做法是把会话行从库里删掉再订阅一次：若仍回表，会以「会话未登记」被拒；
/// 仍能放行说明第二次判定读的是缓存。
#[test]
fn repeated_session_admission_is_served_from_the_cache() {
    let fixture = Fixture::new();
    let session_id = fixture.session(
        "01J8ZQ7K3M4N5P6Q7R8S9T0V4W",
        Some("workspace-a"),
        SessionTrustScope::Private,
    );
    let bridge = fixture.bridge();
    let request = || GatewaySubscriptionRequest {
        workspace_id: "workspace-a".to_owned(),
        interaction_id: String::new(),
        session_id: Some(session_id.as_str().to_owned()),
        resume_cursor: None,
    };

    // 首条订阅必须持有：关闭订阅会立刻回收放行缓存，那是另一个用例要验证的行为。
    let _live = GatewayEgress::subscribe(&bridge, request()).expect("首次订阅应放行");

    let connection = rusqlite::Connection::open(fixture.store.path()).expect("打开运行态库");
    connection
        .execute(
            "DELETE FROM sessions WHERE session_id=?1",
            [session_id.as_str()],
        )
        .expect("删掉会话行，制造「回表就会失败」的条件");

    GatewayEgress::subscribe(&bridge, request()).expect("TTL 内的第二次判定必须命中缓存");
}

/// 缓存有寿命：过期之后必须重新回表，删掉的会话因此会被拒。
#[test]
fn expired_admission_falls_back_to_the_store() {
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Clone)]
    struct StepClock(Arc<AtomicU64>);

    impl RuntimeClock for StepClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::Acquire)
        }
    }

    let fixture = Fixture::new();
    let session_id = fixture.session(
        "01J8ZQ7K3M4N5P6Q7R8S9T0V4X",
        Some("workspace-a"),
        SessionTrustScope::Private,
    );
    let now = Arc::new(AtomicU64::new(NOW_MS));
    let bridge = DaemonGatewayBridge::new(GatewayHostDependencies {
        store: fixture.store.clone(),
        active_turn_steer_sink: Arc::new(NoActiveTurnSteerSink),
        clock: StepClock(Arc::clone(&now)),
        action_lease_duration_ms: ACTION_LEASE_DURATION_MS,
        stream_bridge: Arc::clone(&fixture.stream_bridge),
        host_generation: None,
    });
    let request = || GatewaySubscriptionRequest {
        workspace_id: "workspace-a".to_owned(),
        interaction_id: String::new(),
        session_id: Some(session_id.as_str().to_owned()),
        resume_cursor: None,
    };
    // 首条订阅必须持有：关闭订阅会立刻回收放行缓存，那是另一个用例要验证的行为。
    let _live = GatewayEgress::subscribe(&bridge, request()).expect("首次订阅应放行");

    let connection = rusqlite::Connection::open(fixture.store.path()).expect("打开运行态库");
    connection
        .execute(
            "DELETE FROM sessions WHERE session_id=?1",
            [session_id.as_str()],
        )
        .expect("删掉会话行");
    // 缓存寿终（30 秒）之后再判一次：必须回表，并因会话不存在而被拒。
    now.fetch_add(30_000, Ordering::AcqRel);
    let error = match GatewayEgress::subscribe(&bridge, request()) {
        Ok(_) => panic!("过期后必须回表，删掉的会话不得再被放行"),
        Err(error) => error,
    };
    assert!(matches!(error, GatewayKernelError::Ingress(_)), "{error:?}");
}

/// 关闭订阅即回收放行缓存：之后必须重新回表。
#[test]
fn closing_the_subscription_evicts_the_admission() {
    let fixture = Fixture::new();
    let session_id = fixture.session(
        "01J8ZQ7K3M4N5P6Q7R8S9T0V4Y",
        Some("workspace-a"),
        SessionTrustScope::Private,
    );
    let bridge = fixture.bridge();
    let request = || GatewaySubscriptionRequest {
        workspace_id: "workspace-a".to_owned(),
        interaction_id: String::new(),
        session_id: Some(session_id.as_str().to_owned()),
        resume_cursor: None,
    };

    // 订阅建立后立刻关闭：这条会话的放行判定随之作废。
    drop(GatewayEgress::subscribe(&bridge, request()).expect("首次订阅应放行"));

    let connection = rusqlite::Connection::open(fixture.store.path()).expect("打开运行态库");
    connection
        .execute(
            "DELETE FROM sessions WHERE session_id=?1",
            [session_id.as_str()],
        )
        .expect("删掉会话行");
    let error = match GatewayEgress::subscribe(&bridge, request()) {
        Ok(_) => panic!("关闭订阅后必须回表，删掉的会话不得再被放行"),
        Err(error) => error,
    };
    assert!(matches!(error, GatewayKernelError::Ingress(_)), "{error:?}");
}
