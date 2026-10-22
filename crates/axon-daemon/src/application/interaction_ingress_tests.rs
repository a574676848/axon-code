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
        InteractionId::new("group-interaction").expect("ID"),
        InteractionSource::new("feishu", "group-message").expect("来源"),
        ScopeRef::workspace("group-workspace").expect("作用域"),
        AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace),
        ReplyRoute::new("feishu", "chat-group").expect("路由"),
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
fn completion_and_cron_inputs_stay_independent_of_user_turn() {
    for source_message_id in ["completion-steer-execution-1", "cron:job-1"] {
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
