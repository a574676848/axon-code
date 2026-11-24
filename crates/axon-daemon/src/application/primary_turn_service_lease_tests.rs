//! 主回合服务的幂等与租约：每个 Interaction 只跑一次有效回合，失败如实回绝，长回合持续续租。

use super::*;

#[test]
fn each_interaction_gets_one_effective_primary_turn() {
    let store = TestStore::default();
    store.append(&interaction()).expect("追加成功");
    let runner = RecordingRunner::default();
    let service = PrimaryTurnService::new(
        store.clone(),
        runner.clone(),
        TestClock,
        TEST_LEASE_DURATION_MS,
    )
    .expect("服务创建成功");

    let first = service.tick(10).expect("首次 tick 成功");
    let second = service.tick(10).expect("重复 tick 成功");

    assert_eq!(first.claimed, 1);
    assert_eq!(first.understood, 1);
    assert_eq!(second.claimed, 0);
    assert_eq!(
        runner.calls.lock().expect("锁未中毒").as_slice(),
        ["interaction-primary"]
    );
}

#[test]
fn runner_failure_rejects_interaction_and_does_not_escape_as_success() {
    let store = TestStore::default();
    let interaction = interaction();
    let interaction_id = interaction.interaction_id.clone();
    store.append(&interaction).expect("追加成功");
    let bridge = Arc::new(EventStreamBridge::new());
    let subscription = bridge
        .subscribe(
            primary_session_id_for_interaction(&interaction).expect("Primary Session ID"),
            interaction_id.clone(),
            None,
        )
        .expect("订阅 Interaction 状态");
    let service = PrimaryTurnService::new_with_worker_pool(
        store.clone(),
        FailingRunner,
        TestClock,
        TEST_LEASE_DURATION_MS,
        PrimaryTurnWorkerPoolSlot::default(),
        bridge,
    )
    .expect("服务创建成功");

    let report = service.tick(1).expect("拒绝应被持久化");

    assert_eq!(
        subscription.recv().expect("应收到 rejected 通知"),
        crate::application::event_stream_bridge::StreamNotification::InteractionStatus(
            InteractionStatus::Rejected
        )
    );
    assert_eq!(report.rejected, 1);
    assert_eq!(
        report.failures,
        [PrimaryTurnFailure {
            interaction_id: InteractionId::new("interaction-primary").expect("有效 ID"),
            layer: PrimaryTurnErrorLayer::Provider,
            message: "输入不满足安全策略".to_owned(),
            channel: "tui".to_owned(),
            destination: "terminal-primary".to_owned(),
            source_message_id: "message-primary".to_owned(),
        }]
    );
    assert_eq!(
        store
            .state
            .lock()
            .expect("锁未中毒")
            .as_ref()
            .map(|item| item.0.status),
        Some(InteractionStatus::Rejected)
    );
}

#[test]
fn long_running_primary_turn_renews_interaction_lease() {
    let store = TestStore::default();
    store.append(&interaction()).expect("追加成功");
    let runner = WaitForLeaseRenewalRunner {
        renewals: Arc::clone(&store.renewals),
    };
    let service = PrimaryTurnService::new(
        store.clone(),
        runner.clone(),
        TestClock,
        RENEWAL_TEST_LEASE_DURATION_MS,
    )
    .expect("服务创建成功");

    let report = service.tick(1).expect("长请求主 turn 成功");

    assert_eq!(report.understood, 1);
    assert!(store.renewals.load(Ordering::SeqCst) >= 1);
}
