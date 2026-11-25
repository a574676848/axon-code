//! Interaction ingress 测试。

use super::*;
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionSource, ReplyRoute,
    ScopeRef,
};
use std::sync::{Arc, Mutex};

const TEST_NOW_MS: u64 = 10;
const TEST_LEASE_DURATION_MS: u64 = 1_000;
const TEST_LEASE_TOKEN: &str = "ingress-test-lease";

#[derive(Clone, Default)]
struct Store {
    appended: Arc<Mutex<Vec<InteractionId>>>,
    records: Arc<Mutex<Vec<InteractionRecord>>>,
    claimed: Arc<Mutex<Vec<InteractionId>>>,
    released: Arc<Mutex<Vec<InteractionId>>>,
    understood: Arc<Mutex<Vec<InteractionId>>>,
    inserted: bool,
}

impl InteractionStore for Store {
    fn settle_dispatch(
        &self,
        _interaction_id: &axon_domain::InteractionId,
        _terminal_kind: axon_session::DispatchTerminalKind,
        _now_ms: u64,
    ) -> Result<(), axon_session::InteractionStoreError> {
        Ok(())
    }
    fn append(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<InteractionAppendReceipt, InteractionStoreError> {
        self.appended
            .lock()
            .expect("锁未中毒")
            .push(interaction.interaction_id.clone());
        if self.inserted {
            self.records
                .lock()
                .expect("锁未中毒")
                .push(interaction.clone());
        }
        Ok(InteractionAppendReceipt {
            interaction_id: interaction.interaction_id.clone(),
            inserted: self.inserted,
        })
    }

    fn claim_processing(
        &self,
        _limit: u32,
        _now_ms: u64,
        _lease_duration_ms: u64,
    ) -> Result<Vec<axon_session::InteractionProcessingLease>, InteractionStoreError> {
        Ok(Vec::new())
    }

    fn claim_processing_by_id(
        &self,
        interaction_id: &InteractionId,
        _now_ms: u64,
        _lease_duration_ms: u64,
    ) -> Result<Option<axon_session::InteractionProcessingLease>, InteractionStoreError> {
        self.claimed
            .lock()
            .expect("锁未中毒")
            .push(interaction_id.clone());
        let records = self.records.lock().expect("锁未中毒");
        let Some(record) = records
            .iter()
            .find(|record| &record.interaction_id == interaction_id)
            .cloned()
        else {
            return Ok(None);
        };
        let mut interaction = record;
        interaction.status = InteractionStatus::Processing;
        Ok(Some(axon_session::InteractionProcessingLease {
            interaction,
            lease_token: TEST_LEASE_TOKEN.to_owned(),
            lease_expires_at_ms: TEST_NOW_MS.saturating_add(TEST_LEASE_DURATION_MS),
        }))
    }

    fn renew_processing(
        &self,
        _interaction_id: &InteractionId,
        _lease_token: &str,
        _now_ms: u64,
        _lease_duration_ms: u64,
    ) -> Result<u64, InteractionStoreError> {
        Err(InteractionStoreError::InvalidState(
            "Ingress 测试仓不持有 processing lease".to_owned(),
        ))
    }

    fn release_processing(
        &self,
        interaction_id: &InteractionId,
        _lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        self.released
            .lock()
            .expect("锁未中毒")
            .push(interaction_id.clone());
        Ok(())
    }

    fn requeue_steered(
        &self,
        _interaction_id: &InteractionId,
        _now_ms: u64,
    ) -> Result<bool, InteractionStoreError> {
        // 测试仓不建归档表：真实实现重投的是归档行，这里如实返回「没有可重投的行」。
        Ok(false)
    }

    fn mark_understood(
        &self,
        interaction_id: &InteractionId,
        _lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        self.understood
            .lock()
            .expect("锁未中毒")
            .push(interaction_id.clone());
        Ok(())
    }

    fn mark_rejected(
        &self,
        _interaction_id: &InteractionId,
        _lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        Ok(())
    }

    fn mark_interrupted(
        &self,
        _interaction_id: &InteractionId,
        _lease_token: &str,
        _now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        Ok(())
    }
}

#[derive(Clone)]
struct Sink {
    calls: Arc<Mutex<Vec<InteractionId>>>,
    result: Result<bool, String>,
}

impl ActiveTurnSteerSink for Sink {
    fn try_steer(&self, interaction: &InteractionRecord) -> Result<bool, String> {
        self.calls
            .lock()
            .expect("锁未中毒")
            .push(interaction.interaction_id.clone());
        self.result.clone()
    }
}

fn interaction() -> InteractionRecord {
    InteractionRecord::new(
        axon_domain::DaemonSessionId::new("01J8XQ7K3M4N5P6Q7R8S9T0V1W").expect("合法会话 ID"),
        InteractionId::new("ingress-interaction").expect("ID"),
        InteractionSource::new("tui", "ingress-message").expect("来源"),
        ScopeRef::workspace("ingress-workspace").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", "ingress-terminal").expect("路由"),
        "继续处理",
        1,
    )
    .expect("Interaction")
}

fn group_interaction() -> InteractionRecord {
    InteractionRecord::new(
        axon_domain::DaemonSessionId::new("01J8XQ7K3M4N5P6Q7R8S9T0V1W").expect("合法会话 ID"),
        InteractionId::new("group-interaction").expect("ID"),
        InteractionSource::new("webhook", "group-message").expect("来源"),
        ScopeRef::workspace("group-workspace").expect("作用域"),
        AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace),
        ReplyRoute::new("webhook", "chat-group").expect("路由"),
        "群聊进展",
        2,
    )
    .expect("Interaction")
}

fn ingress(store: Store, sink: Sink) -> PrimaryTurnIngress<Store, Sink> {
    PrimaryTurnIngress::new(store, sink, TEST_NOW_MS, TEST_LEASE_DURATION_MS)
}

#[derive(Clone)]
struct AppendSink {
    primary_interaction_id: Option<InteractionId>,
}

impl ActiveTurnSteerSink for AppendSink {
    fn try_steer(&self, _interaction: &InteractionRecord) -> Result<bool, String> {
        Ok(false)
    }

    fn append_turn_input(
        &self,
        _interaction: &InteractionRecord,
    ) -> Result<Option<InteractionId>, String> {
        Ok(self.primary_interaction_id.clone())
    }
}

#[test]
fn matching_active_turn_reuses_primary_interaction_without_append() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let primary_interaction_id = InteractionId::new("primary-interaction").expect("ID");
    let ingress = PrimaryTurnIngress::new(
        store.clone(),
        AppendSink {
            primary_interaction_id: Some(primary_interaction_id.clone()),
        },
        TEST_NOW_MS,
        TEST_LEASE_DURATION_MS,
    );

    let receipt = ingress.route_input(&interaction()).expect("追加输入成功");

    assert_eq!(receipt.interaction_id, primary_interaction_id);
    assert!(!receipt.inserted);
    assert_eq!(receipt.delivery, InteractionDelivery::Steered);
    assert!(store.appended.lock().expect("锁未中毒").is_empty());
    assert!(store.claimed.lock().expect("锁未中毒").is_empty());
    assert!(store.released.lock().expect("锁未中毒").is_empty());
}

#[test]
fn route_without_active_turn_appends_without_claim_or_release() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let ingress = PrimaryTurnIngress::new(
        store.clone(),
        AppendSink {
            primary_interaction_id: None,
        },
        TEST_NOW_MS,
        TEST_LEASE_DURATION_MS,
    );

    let receipt = ingress
        .route_input(&interaction())
        .expect("新 turn 应进入 durable queue");

    assert!(receipt.inserted);
    assert_eq!(receipt.delivery, InteractionDelivery::Queued);
    assert_eq!(store.appended.lock().expect("锁未中毒").len(), 1);
    assert!(store.claimed.lock().expect("锁未中毒").is_empty());
    assert!(store.released.lock().expect("锁未中毒").is_empty());
    assert!(store.understood.lock().expect("锁未中毒").is_empty());
}

#[test]
fn system_input_stays_independent_of_user_turn() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let ingress = PrimaryTurnIngress::new(
        store.clone(),
        AppendSink {
            primary_interaction_id: Some(InteractionId::new("user-primary").expect("ID")),
        },
        TEST_NOW_MS,
        TEST_LEASE_DURATION_MS,
    );
    let mut system = interaction();
    system.source = InteractionSource::new("system", "cron:job:1").expect("来源");
    system.reply_route = ReplyRoute::new("system", "workspace").expect("路由");

    let receipt = ingress.route_input(&system).expect("系统输入应独立提交");

    assert!(receipt.inserted);
    assert_eq!(store.appended.lock().expect("锁未中毒").len(), 1);
}

#[test]
fn completion_and_wake_inputs_stay_independent_of_user_turn() {
    for source_message_id in [
        "completion-steer-execution-1",
        "cron-wake:job-1",
        "shell-job-wake:job-1",
    ] {
        let store = Store {
            inserted: true,
            ..Store::default()
        };
        let ingress = PrimaryTurnIngress::new(
            store.clone(),
            AppendSink {
                primary_interaction_id: Some(InteractionId::new("user-primary").expect("ID")),
            },
            TEST_NOW_MS,
            TEST_LEASE_DURATION_MS,
        );
        let mut system = interaction();
        system.source = InteractionSource::new("tui", source_message_id).expect("来源");

        let receipt = ingress.route_input(&system).expect("系统输入应独立提交");

        assert!(receipt.inserted);
        assert_eq!(store.appended.lock().expect("锁未中毒").len(), 1);
    }
}

#[test]
fn durable_append_happens_before_active_steer_and_failure_falls_back_to_queue() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let sink = Sink {
        calls: Arc::new(Mutex::new(Vec::new())),
        result: Err("当前 turn 已到安全边界".to_owned()),
    };
    let ingress = ingress(store.clone(), sink.clone());

    let receipt = ingress.submit(&interaction()).expect("ingress 成功");

    assert_eq!(receipt.delivery, InteractionDelivery::Queued);
    assert_eq!(
        receipt.steer_error.as_deref(),
        Some("当前 turn 已到安全边界")
    );
    assert_eq!(store.appended.lock().expect("锁未中毒").len(), 1);
    assert_eq!(store.released.lock().expect("锁未中毒").len(), 1);
    assert!(store.understood.lock().expect("锁未中毒").is_empty());
    assert_eq!(sink.calls.lock().expect("锁未中毒").len(), 1);
}

#[test]
fn successful_steer_settles_claim_as_understood() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let sink = Sink {
        calls: Arc::new(Mutex::new(Vec::new())),
        result: Ok(true),
    };
    let ingress = ingress(store.clone(), sink);

    let receipt = ingress.submit(&interaction()).expect("steer 成功");

    assert_eq!(receipt.delivery, InteractionDelivery::Steered);
    assert_eq!(store.claimed.lock().expect("锁未中毒").len(), 1);
    assert_eq!(store.understood.lock().expect("锁未中毒").len(), 1);
    assert!(store.released.lock().expect("锁未中毒").is_empty());
}

#[test]
fn duplicate_interaction_is_not_steered_again() {
    let store = Store {
        inserted: false,
        ..Store::default()
    };
    let sink = Sink {
        calls: Arc::new(Mutex::new(Vec::new())),
        result: Ok(true),
    };
    let ingress = ingress(store, sink.clone());

    let receipt = ingress.submit(&interaction()).expect("重复 ingress 成功");

    assert!(!receipt.inserted);
    assert_eq!(receipt.delivery, InteractionDelivery::Queued);
    assert!(sink.calls.lock().expect("锁未中毒").is_empty());
}

#[test]
fn private_and_group_fixtures_keep_isolated_audience_and_route() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let sink = Sink {
        calls: Arc::new(Mutex::new(Vec::new())),
        result: Ok(false),
    };
    let ingress = ingress(store.clone(), sink);

    let private_receipt = ingress.submit(&interaction()).expect("私聊应通过策略");
    let group_receipt = ingress
        .submit(&group_interaction())
        .expect("群聊应通过策略");

    assert!(private_receipt.inserted);
    assert!(group_receipt.inserted);
    assert_eq!(store.appended.lock().expect("锁未中毒").len(), 2);
}

#[test]
fn ingress_policy_rejects_cross_audience_and_route_leaks_before_append() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let ingress = ingress(
        store.clone(),
        Sink {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: Ok(false),
        },
    );
    let mut private_with_group_disclosure = interaction();
    private_with_group_disclosure.audience =
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::Workspace);
    let error = ingress
        .submit(&private_with_group_disclosure)
        .expect_err("私聊不得扩大披露边界");
    assert!(error.to_string().contains("audience"));

    let mut mismatched_route = group_interaction();
    mismatched_route.reply_route = ReplyRoute::new("cli", "terminal").expect("路由");
    let error = ingress
        .submit(&mismatched_route)
        .expect_err("回复渠道不得跨入口");
    assert!(error.to_string().contains("reply route"));

    assert!(store.appended.lock().expect("锁未中毒").is_empty());
}

#[test]
fn ingress_policy_rejects_global_scope() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let ingress = ingress(
        store,
        Sink {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: Ok(false),
        },
    );
    let mut global = interaction();
    global.scope = ScopeRef::global();
    let error = ingress.submit(&global).expect_err("入口必须绑定 Workspace");
    assert!(error.to_string().contains("Workspace"));
}

#[test]
fn ingress_policy_rejects_non_received_initial_status() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let ingress = ingress(
        store,
        Sink {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: Ok(false),
        },
    );
    let mut forged = interaction();
    forged.status = InteractionStatus::Understood;

    let error = ingress.submit(&forged).expect_err("入站不得伪造终态");
    assert!(error.to_string().contains("received"));
}

#[test]
fn system_completion_input_is_never_steered_into_active_turn() {
    let store = Store {
        inserted: true,
        ..Store::default()
    };
    let sink = Sink {
        calls: Arc::new(Mutex::new(Vec::new())),
        result: Ok(true),
    };
    let ingress = ingress(store.clone(), sink.clone());
    let mut completion = interaction();
    completion.source =
        InteractionSource::new("desktop", "completion-steer-execution-1").expect("来源");
    completion.reply_route = ReplyRoute::new("desktop", "ingress-workbench").expect("路由");

    let receipt = ingress
        .submit_new_interaction(&completion)
        .expect("完成通知应落成独立排队轮次");

    assert!(receipt.inserted);
    assert_eq!(receipt.delivery, InteractionDelivery::Queued);
    assert_eq!(
        sink.calls.lock().expect("锁未中毒").len(),
        1,
        "活跃轮只允许收到一次即时提示"
    );
    assert_eq!(
        store.released.lock().expect("锁未中毒").len(),
        1,
        "已领取的租约必须放回队列等待主 turn 领取"
    );
    assert!(
        store.understood.lock().expect("锁未中毒").is_empty(),
        "并入活跃轮不得把系统输入结算成已理解"
    );
}

/// W8.1 探针：同一投递目标、三个不同渠道，只允许落进同一条会话。
///
/// 会话身份由入站显式给出，渠道与投递目标都不参与派生；这条用例守的是「没有任何一层
/// 按渠道或目标另建一条会话」。
#[test]
fn three_channels_with_the_same_destination_land_in_one_session() {
    use axon_session::{InteractionQueryStore as _, RuntimeStore as _, SqliteRuntimeStore};

    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id =
        axon_domain::DaemonSessionId::new("01J8ZQ7K3M4N5P6Q7R8S9T0V1W").expect("会话标识");
    store
        .create_session(&session_id, ".", TEST_NOW_MS)
        .expect("建会话");
    let ingress = PrimaryTurnIngress::new(
        store.clone(),
        super::NoActiveTurnSteerSink,
        TEST_NOW_MS,
        TEST_LEASE_DURATION_MS,
    );

    for (index, channel) in ["desktop", "cli", "tui"].iter().enumerate() {
        let interaction = InteractionRecord::new(
            session_id.clone(),
            InteractionId::new(format!("01J8ZQ7K3M4N5P6Q7R8S9T0V2{index}")).expect("交互标识"),
            InteractionSource::new(*channel, format!("message-{index}"))
                .and_then(|source| source.with_client(*channel))
                .expect("来源"),
            ScopeRef::workspace("workspace-a").expect("作用域"),
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            // 三渠道共用同一个投递目标：旧模型会按它与渠道派生出三条会话。
            ReplyRoute::new(*channel, "workbench-1").expect("回复路由"),
            "继续处理",
            TEST_NOW_MS + u64::try_from(index).expect("序号"),
        )
        .expect("Interaction");
        let receipt = ingress.route_input(&interaction).expect("入站应成功");
        assert!(receipt.inserted, "三条入站都必须是新记录");
    }

    let rows = store
        .interactions_for_session(session_id.as_str(), 10, None)
        .expect("读取会话入站");
    assert_eq!(rows.len(), 3, "三个渠道的三条入站都应落在同一条会话里");
    assert!(
        rows.iter().all(|row| row.session_id == session_id),
        "任何一条都不得被改投到别的会话"
    );
    assert!(
        rows.iter()
            .all(|row| row.reply_route.destination() == "workbench-1"),
        "三条入站共用同一个投递目标"
    );
    let sessions: i64 = store
        .with_read_connection(|connection| {
            connection.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        })
        .expect("统计会话");
    assert_eq!(sessions, 1, "不得按渠道或投递目标另建会话");
}
