//! 主回合服务的幂等与租约：每个 Interaction 只跑一次有效回合，失败如实回绝，长回合持续续租。

use super::*;

/// 租约丢失用例的租约时长：续租周期是它的三分之一，用例只需等几拍。
const LEASE_LOSS_TEST_LEASE_DURATION_MS: u64 = 30;
/// 等续租线程撞上「行已不属于我们」的时长：约六个续租周期。
const LEASE_LOSS_WAIT: std::time::Duration = std::time::Duration::from_millis(120);

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

/// 租约被证明不在自己手上时，必须请运行侧收口，而不是继续跑到收尾。
///
/// 行被别的路径收口之后，这一轮已经不可能再写入任何事件（事件面按租约令牌与到期时间
/// 做围栏）。旧实现只是把续租线程停掉，运行侧照旧跑到最后，token 烧在一个再也写不进去
/// 的轮次上，用户那边多出一段落不了地的输出。
#[test]
fn lease_loss_asks_the_runner_to_settle_the_turn() {
    let store = TestStore::default();
    store.append(&interaction()).expect("追加成功");
    let expected_session = primary_session_id_for_interaction(&interaction())
        .expect("Primary Session ID")
        .as_str()
        .to_owned();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let service = PrimaryTurnService::new(
        store.clone(),
        StealLeaseRunner {
            store: store.clone(),
        },
        TestClock,
        LEASE_LOSS_TEST_LEASE_DURATION_MS,
    )
    .expect("服务创建成功")
    .with_lease_lost(LeaseLostHook::new({
        let calls = Arc::clone(&calls);
        move |session_id: &axon_domain::DaemonSessionId| {
            calls
                .lock()
                .expect("锁未中毒")
                .push(session_id.as_str().to_owned());
        }
    }));

    // 本轮随后会如实失败：租约已经丢了，收口不该被记成一次成功。
    let _ = service.tick(1);

    let calls = calls.lock().expect("锁未中毒").clone();
    assert_eq!(
        calls,
        vec![expected_session],
        "租约丢失必须按会话请求收口一次"
    );
}

/// 运行中把这一行从自己手里拿走，模拟 supervisor 的收敛。
#[derive(Clone)]
struct StealLeaseRunner {
    store: TestStore,
}

impl PrimaryTurnRunner for StealLeaseRunner {
    fn run(&self, _request: PrimaryTurnRequest) -> Result<(), PrimaryTurnRunnerError> {
        {
            let mut state = self.store.state.lock().expect("锁未中毒");
            if let Some((record, _token)) = state.as_mut() {
                record.status = InteractionStatus::Interrupted;
            }
        }
        // 等到续租线程至少撞上一次「行已不属于我们」。
        std::thread::sleep(LEASE_LOSS_WAIT);
        Ok(())
    }
}
