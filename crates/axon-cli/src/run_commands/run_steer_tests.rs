//! turn-local 追加队列与活跃主 turn 注册表的单元测试。
//!
//! 边界语义（关闭即拒、重开即收、克隆不延长生命周期）与容量预算都在这里钉住；
//! 它们由 `run_steer.rs` 以 `#[path]` 挂载，保持生产文件在单文件行数上限内。

use super::*;

#[test]
fn close_and_drain_atomically_rejects_late_input_until_reopened() {
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    bridge.try_send("第一条".to_owned()).unwrap();

    let drained = inbox.close_and_drain();

    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].message.content, "第一条");
    assert_eq!(
        bridge.try_send("过晚输入".to_owned()),
        Err(RunSteerSendError::Inactive)
    );
    inbox.reopen();
    bridge.try_send("第二条".to_owned()).unwrap();
    assert_eq!(inbox.drain()[0].message.content, "第二条");
}

#[test]
fn unconsumed_steers_are_handed_back_when_the_turn_closes() {
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    let handed_back: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    inbox.install_requeue_hook(Some(Arc::new({
        let handed_back = Arc::clone(&handed_back);
        move |interaction_id: &InteractionId, _now_ms: u64| {
            handed_back
                .lock()
                .expect("锁未中毒")
                .push(interaction_id.as_str().to_owned());
            Ok(true)
        }
    })));

    inbox
        .try_send_input(
            AgentTurnSteerInput {
                message: AgentModelMessage::text("user", "补充约束"),
                quoted_message: None,
            },
            Some(InteractionId::new("interaction-steer").expect("Interaction ID")),
        )
        .expect("收下已结算的追加");
    // 非边界出口：这一轮直接收口，没有取走它。
    drop(inbox);

    assert_eq!(
        handed_back.lock().expect("锁未中毒").as_slice(),
        ["interaction-steer"],
        "没被取走的入站事实必须交还调用方重投"
    );
}

#[test]
fn consumed_steers_are_not_handed_back() {
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    let handed_back: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    inbox.install_requeue_hook(Some(Arc::new({
        let handed_back = Arc::clone(&handed_back);
        move |_interaction_id: &InteractionId, _now_ms: u64| {
            *handed_back.lock().expect("锁未中毒") += 1;
            Ok(true)
        }
    })));

    inbox
        .try_send_input(
            AgentTurnSteerInput {
                message: AgentModelMessage::text("user", "补充约束"),
                quoted_message: None,
            },
            Some(InteractionId::new("interaction-consumed").expect("Interaction ID")),
        )
        .expect("收下已结算的追加");
    let taken = inbox.drain();
    assert_eq!(taken.len(), 1);
    drop(inbox);

    assert_eq!(
        *handed_back.lock().expect("锁未中毒"),
        0,
        "已经取走并注入的追加不得再回投，否则同一句话会跑两轮"
    );
}

#[test]
fn registry_installs_the_requeue_hook_into_registered_turns() {
    let registry = ActivePrimaryTurnRegistry::default();
    let handed_back: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    registry.install_requeue_hook(Arc::new({
        let handed_back = Arc::clone(&handed_back);
        move |_interaction_id: &InteractionId, _now_ms: u64| {
            *handed_back.lock().expect("锁未中毒") += 1;
            Ok(true)
        }
    }));
    let key = ActivePrimaryTurnRegistry::affinity_key("session-hook");
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    let registration = registry
        .register(
            &key,
            InteractionId::new("interaction-turn").expect("Interaction ID"),
            inbox.clone(),
            RunCancelToken::default(),
        )
        .expect("注册活跃主 turn");

    registry
        .try_send_steer(
            &key,
            InteractionId::new("interaction-steer").expect("Interaction ID"),
            AgentModelMessage::text("user", "补充约束"),
        )
        .expect("注册表收下追加")
        .expect("必须命中活跃 turn");
    drop(inbox);

    assert_eq!(
        *handed_back.lock().expect("锁未中毒"),
        1,
        "注册时装配的落点必须传到本轮队列，否则收口回投等于没接"
    );
    drop(registration);
}

#[test]
fn unconsumed_steer_returns_to_the_durable_queue() {
    use axon_session::{InteractionStore as _, RuntimeStore as _, SqliteRuntimeStore};

    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    store.initialize().expect("初始化 schema");
    let interaction = axon_domain::InteractionRecord::new(
        axon_domain::DaemonSessionId::new("session-requeue-e2e").expect("合法会话 ID"),
        InteractionId::new("interaction-requeue-e2e").expect("合法 ID"),
        axon_domain::InteractionSource::new("desktop", "message-requeue-e2e").expect("合法来源"),
        axon_domain::ScopeRef::workspace("workspace-requeue").expect("合法作用域"),
        axon_domain::AudienceContext::new(
            axon_domain::AudienceKind::Private,
            axon_domain::DisclosurePolicy::PrivateOnly,
        ),
        axon_domain::ReplyRoute::new("desktop", "workbench-requeue").expect("合法回复路由"),
        "补充约束",
        10,
    )
    .expect("合法 Interaction");
    store.append(&interaction).expect("入站落库");
    // 入站侧命中活跃 turn：立刻领取并结算成 understood。
    let lease = store
        .claim_processing_by_id(&interaction.interaction_id, 20, 100)
        .expect("按 ID 领取")
        .expect("首次领取必须成功");
    store
        .mark_understood(&interaction.interaction_id, &lease.lease_token, 30)
        .expect("结算 understood");
    assert!(
        store.claim_processing(1, 40, 100).expect("领取").is_empty(),
        "结算之后调度器本就不该再领到它"
    );

    // 生产装配：注册表装上真实存储落点，本轮收下这条已结算的追加却没取走。
    let registry = ActivePrimaryTurnRegistry::default();
    registry.install_requeue_hook(crate::daemon_commands::store_requeue_hook(store.clone()));
    let key = ActivePrimaryTurnRegistry::affinity_key("session-requeue-e2e");
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    let registration = registry
        .register(
            &key,
            InteractionId::new("interaction-turn-e2e").expect("Interaction ID"),
            inbox.clone(),
            RunCancelToken::default(),
        )
        .expect("注册活跃主 turn");
    registry
        .try_send_steer(
            &key,
            interaction.interaction_id.clone(),
            AgentModelMessage::text("user", "补充约束"),
        )
        .expect("收下追加")
        .expect("必须命中活跃 turn");

    // 非边界出口：本轮直接收口，一条都没取走。
    drop(inbox);

    let claimed = store.claim_processing(1, 50, 100).expect("重投后领取");
    assert_eq!(claimed.len(), 1, "没被取走的追加必须回到 durable 队列");
    assert_eq!(
        claimed[0].interaction.interaction_id, interaction.interaction_id,
        "重投回来的必须正是原来那一条"
    );
    drop(registration);
}

#[test]
fn user_interrupt_leaves_the_carry_over_to_the_client() {
    let registry = ActivePrimaryTurnRegistry::default();
    let handed_back: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    registry.install_requeue_hook(Arc::new({
        let handed_back = Arc::clone(&handed_back);
        move |_interaction_id: &InteractionId, _now_ms: u64| {
            *handed_back.lock().expect("锁未中毒") += 1;
            Ok(true)
        }
    }));
    let key = ActivePrimaryTurnRegistry::affinity_key("session-user-interrupt");
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    let registration = registry
        .register(
            &key,
            InteractionId::new("interaction-turn-user").expect("Interaction ID"),
            inbox.clone(),
            RunCancelToken::default(),
        )
        .expect("注册活跃主 turn");
    registry
        .try_send_steer(
            &key,
            InteractionId::new("interaction-steer-user").expect("Interaction ID"),
            AgentModelMessage::text("user", "补充约束"),
        )
        .expect("注册表收下追加")
        .expect("必须命中活跃 turn");

    // 用户按 ESC：桌面端会把这条转投下一条。
    registry
        .interrupt_for_user(&key)
        .expect("用户中断")
        .expect("必须命中活跃 turn");
    drop(inbox);

    assert_eq!(
        *handed_back.lock().expect("锁未中毒"),
        0,
        "用户中断时客户端会转投，服务端再重投就是同一句话跑两轮"
    );
    drop(registration);
}

#[test]
fn cloned_inbox_does_not_extend_or_close_active_lifecycle() {
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    let observer = inbox.clone();

    drop(observer);
    bridge.try_send("仍在运行".to_owned()).unwrap();
    drop(inbox);

    assert_eq!(
        bridge.try_send("运行已结束".to_owned()),
        Err(RunSteerSendError::Inactive)
    );
}

#[test]
fn capacity_overflow_is_rejected_and_drain_releases_budget() {
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();

    assert_eq!(
        bridge.try_send("x".repeat(RUN_STEER_PENDING_BYTE_LIMIT)),
        Err(RunSteerSendError::CapacityExceeded)
    );
    for index in 0..RUN_STEER_PENDING_MESSAGE_LIMIT {
        bridge.try_send(format!("追加消息 {index}")).unwrap();
    }
    assert_eq!(
        bridge.try_send("超限消息".to_owned()),
        Err(RunSteerSendError::CapacityExceeded)
    );

    assert_eq!(inbox.drain().len(), RUN_STEER_PENDING_MESSAGE_LIMIT);
    bridge.try_send("释放后可继续".to_owned()).unwrap();
    assert_eq!(inbox.drain()[0].message.content, "释放后可继续");
}

#[test]
fn deferred_runtime_note_uses_internal_message_and_registration_lifecycle() {
    let session_key = format!("deferred-diagnostics-{}", std::process::id());
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    let registration = register_deferred_message_session(&session_key, inbox.clone());

    try_send_deferred_runtime_note(
        &session_key,
        AgentRuntimeNoteKind::DeferredDiagnostics,
        "late diagnostics".to_owned(),
    )
    .unwrap();

    let input = inbox.drain().pop().unwrap();
    assert!(input.message.cache_excluded);
    assert!(input.message.content.contains("late diagnostics"));
    drop(registration);
    assert_eq!(
        try_send_deferred_runtime_note(
            &session_key,
            AgentRuntimeNoteKind::DeferredDiagnostics,
            "too late".to_owned(),
        ),
        Err(RunSteerSendError::Inactive)
    );
}

#[test]
fn active_primary_turn_registry_routes_by_workspace_and_unregisters() {
    let registry = ActivePrimaryTurnRegistry::default();
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    let interaction_id = InteractionId::new("interaction-a").expect("Interaction ID");
    let affinity_key = ActivePrimaryTurnRegistry::affinity_key("session-session-a");
    let registration = registry
        .register(
            &affinity_key,
            interaction_id.clone(),
            inbox.clone(),
            RunCancelToken::default(),
        )
        .expect("注册活跃主 turn");
    assert_eq!(registry.active_count(), 1);

    assert!(
        registry
            .try_send(&affinity_key, "补充约束".to_owned())
            .expect("steer 成功")
            .is_some_and(|value| value == interaction_id)
    );
    assert_eq!(inbox.drain()[0].message.content, "补充约束");
    assert!(
        !registry
            .try_send(
                &ActivePrimaryTurnRegistry::affinity_key("session-session-b"),
                "其他工作区".to_owned(),
            )
            .expect("其他工作区应排队")
            .is_some()
    );
    assert!(
        !registry
            .try_send(
                &ActivePrimaryTurnRegistry::affinity_key("session-session-a-remote"),
                "其他渠道".to_owned(),
            )
            .expect("其他渠道应隔离")
            .is_some()
    );

    drop(registration);
    assert_eq!(registry.active_count(), 0);
    assert!(
        !registry
            .try_send(&affinity_key, "过晚输入".to_owned())
            .expect("注册释放后应排队")
            .is_some()
    );
}
