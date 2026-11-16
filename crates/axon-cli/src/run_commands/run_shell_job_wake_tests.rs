use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axon_agent_runtime::{
    AgentTurnSteerSource, CompletionNotice, JOB_REASON_CANCELLED, JobProducerKind, JobTerminalKind,
    RunCancelToken, job_runtime::JOB_REASON_USER_STOP, publish_completion_notice,
    publish_completion_notices,
};
use axon_daemon::{
    PrimaryTurnRequest, PrimaryTurnRunner, PrimaryTurnRunnerError, PrimaryTurnService,
};
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, InteractionStatus, ReplyRoute, SHELL_JOB_WAKE_SOURCE_PREFIX,
    SYSTEM_SOURCE_CHANNEL, ScopeRef,
};
use axon_session::{InteractionQueryStore, InteractionStore, SqliteRuntimeStore};
use axon_session_runtime::RuntimeClock;

use super::{ShellJobWakeBridge, WakeRouteInfo};
use crate::run_commands::run_steer::{
    ActivePrimaryTurnRegistry, RunSteerBridge, register_deferred_message_session,
};
use crate::user_question::turn_steerer::PrimaryTurnWakeHook;

const TEST_LEASE_MS: u64 = 120_000;
const TICK_BATCH: u32 = 32;

#[derive(Clone, Copy)]
struct TestClock;

impl RuntimeClock for TestClock {
    fn now_ms(&self) -> u64 {
        axon_time::unix_now_ms()
    }
}

#[derive(Clone, Default)]
struct RecordingRunner {
    rounds: Arc<Mutex<Vec<PrimaryTurnRequest>>>,
}

impl RecordingRunner {
    fn round_count(&self) -> usize {
        self.rounds.lock().expect("锁未中毒").len()
    }

    fn rounds(&self) -> Vec<PrimaryTurnRequest> {
        self.rounds.lock().expect("锁未中毒").clone()
    }
}

impl PrimaryTurnRunner for RecordingRunner {
    fn run(&self, request: PrimaryTurnRequest) -> Result<(), PrimaryTurnRunnerError> {
        self.rounds.lock().expect("锁未中毒").push(request);
        Ok(())
    }
}

struct TestFixture {
    store: SqliteRuntimeStore,
    wakes: Arc<AtomicUsize>,
    bridge: ShellJobWakeBridge,
    session_id: String,
}

impl TestFixture {
    fn new(channel: &str, destination: &str, workspace_id: &str) -> Self {
        let store_root = axon_test_support::test_temp_root()
            .join(format!("shell-job-wake-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&store_root).expect("创建测试目录");
        let store = SqliteRuntimeStore::from_root(store_root);
        let active_primary_turns = ActivePrimaryTurnRegistry::default();
        let wakes = Arc::new(AtomicUsize::new(0));
        let wake_counter = Arc::clone(&wakes);
        let wake_hook: PrimaryTurnWakeHook = Arc::new(move || {
            wake_counter.fetch_add(1, Ordering::AcqRel);
            Ok(())
        });
        let seed = InteractionRecord::new(
            InteractionId::new(format!("interaction-seed-{}", uuid::Uuid::new_v4())).expect("ID"),
            InteractionSource::new(channel, format!("msg-{}", uuid::Uuid::new_v4())).expect("来源"),
            ScopeRef::workspace(workspace_id).expect("作用域"),
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            ReplyRoute::new(channel, destination).expect("路由"),
            "启动后台作业",
            1,
        )
        .expect("Interaction");
        let session_id = axon_session::interaction_session_id(&seed).expect("车道 ID");
        store.append(&seed).expect("种子入库");
        let service = PrimaryTurnService::new(
            store.clone(),
            RecordingRunner::default(),
            TestClock,
            TEST_LEASE_MS,
        )
        .expect("服务装配");
        let seeded = service.tick(TICK_BATCH).expect("领取种子回合");
        assert_eq!(seeded.claimed, 1);
        let bridge = ShellJobWakeBridge::new(
            store.clone(),
            active_primary_turns,
            TEST_LEASE_MS,
            wake_hook,
        );
        Self {
            store,
            wakes,
            bridge,
            session_id,
        }
    }

    fn wake_count(&self) -> usize {
        self.wakes.load(Ordering::Acquire)
    }

    fn tick(&self, runner: &RecordingRunner) -> axon_daemon::PrimaryTurnTickReport {
        PrimaryTurnService::new(self.store.clone(), runner.clone(), TestClock, TEST_LEASE_MS)
            .expect("服务装配")
            .tick(TICK_BATCH)
            .expect("主 turn 领取")
    }

    fn session_interactions(&self) -> Vec<InteractionRecord> {
        self.store
            .interactions_for_session(&self.session_id, 20, None)
            .expect("读取车道 Interaction")
    }
}

fn shell_notice(
    owner: &str,
    key: &str,
    terminal: JobTerminalKind,
    exit_reason: &str,
) -> CompletionNotice {
    CompletionNotice::new(
        JobProducerKind::Shell,
        key.to_owned(),
        format!("epoch:{key}"),
        owner.to_owned(),
        terminal,
        exit_reason,
        "",
        Some("/tmp/tee/job-1.shell.log".to_owned()),
        None,
    )
}

#[test]
fn active_turn_receives_completion_notice_in_band_without_opening_wake_turn() {
    let fixture = TestFixture::new("webhook", "oc_chat_active", "ws-active");
    let owner = format!("run-active-{}", uuid::Uuid::new_v4());
    let cancel_token = RunCancelToken::default();
    fixture.bridge.register_turn(
        &owner,
        WakeRouteInfo {
            session_id: fixture.session_id.clone(),
            model: Some("gpt-6-astra".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            turn_cancel_token: cancel_token,
        },
    );
    let steer_bridge = RunSteerBridge::default();
    let inbox = steer_bridge.open_with_persistent_completion_wake(true);
    let _registration = register_deferred_message_session(&owner, inbox.clone());

    publish_completion_notice(shell_notice(
        &owner,
        &format!("shell:active:{}", uuid::Uuid::new_v4()),
        JobTerminalKind::Completed,
        "正常结束",
    ));

    let drained = inbox.drain();
    assert_eq!(drained.len(), 1, "活跃回合必须实时收到完成通知");
    assert!(drained[0].message.cache_excluded);
    assert!(drained[0].message.content.contains("任务已完成"));
    assert_eq!(
        fixture.wake_count(),
        0,
        "活跃回合内不得额外发起跨 Turn 唤醒"
    );
}

#[test]
fn ended_turn_wakes_new_turn_preserving_model_and_effort_for_shell_job() {
    let fixture = TestFixture::new("webhook", "oc_chat_wake", "ws-wake");
    let owner = format!("run-ended-{}", uuid::Uuid::new_v4());
    let cancel_token = RunCancelToken::default();
    fixture.bridge.register_turn(
        &owner,
        WakeRouteInfo {
            session_id: fixture.session_id.clone(),
            model: Some("gpt-6-astra".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            turn_cancel_token: cancel_token,
        },
    );
    // 重复登记同一 owner 只更新路由，不得建立第二份订阅。
    fixture.bridge.register_turn(
        &owner,
        WakeRouteInfo {
            session_id: fixture.session_id.clone(),
            model: Some("gpt-6-astra".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            turn_cancel_token: RunCancelToken::default(),
        },
    );

    // 原回合已退出（未注册到 RUN_DEFERRED_MESSAGE_REGISTRY），随后 Shell 作业完成。
    let notice_key = format!("shell:ended:{}", uuid::Uuid::new_v4());
    publish_completion_notice(shell_notice(
        &owner,
        &notice_key,
        JobTerminalKind::Completed,
        "正常结束",
    ));

    assert_eq!(fixture.wake_count(), 1, "原回合结束后必须唤醒一次主 turn");
    let runner = RecordingRunner::default();
    let tick = fixture.tick(&runner);
    assert_eq!(tick.claimed, 1, "必须只领取到一条唤醒 Interaction");
    assert_eq!(runner.round_count(), 1);

    let wake_turn = &runner.rounds()[0];
    assert!(
        wake_turn
            .interaction
            .source
            .source_message_id()
            .starts_with(SHELL_JOB_WAKE_SOURCE_PREFIX)
    );
    assert_eq!(wake_turn.interaction.source.channel(), "webhook");
    assert_eq!(
        wake_turn.interaction.reply_route.destination(),
        "oc_chat_wake"
    );
    assert_eq!(wake_turn.interaction.model.as_deref(), Some("gpt-6-astra"));
    assert_eq!(
        wake_turn.interaction.reasoning_effort.as_deref(),
        Some("high")
    );
    assert!(
        wake_turn
            .interaction
            .content
            .contains("/tmp/tee/job-1.shell.log")
    );
    assert_eq!(
        fixture
            .store
            .get_interaction(&wake_turn.interaction.interaction_id)
            .expect("读取")
            .expect("存在")
            .status,
        InteractionStatus::Understood
    );
}

#[test]
fn cancelled_turn_does_not_wake_but_subagent_and_shell_terminals_do() {
    let fixture = TestFixture::new("desktop", "workbench-1", "ws-cancel");
    let owner = format!("run-cancel-{}", uuid::Uuid::new_v4());
    let cancel_token = RunCancelToken::default();
    cancel_token.cancel();
    fixture.bridge.register_turn(
        &owner,
        WakeRouteInfo {
            session_id: fixture.session_id.clone(),
            model: None,
            reasoning_effort: None,
            turn_cancel_token: cancel_token,
        },
    );

    // 1. 父 turn 取消导致的 Killed（无论是 token 已置位还是原因为 JOB_REASON_CANCELLED）均不得唤醒。
    publish_completion_notice(shell_notice(
        &owner,
        &format!("shell:killed-cancel:{}", uuid::Uuid::new_v4()),
        JobTerminalKind::Killed,
        JOB_REASON_CANCELLED,
    ));
    assert_eq!(fixture.wake_count(), 0, "父 turn 取消导致的终态不得唤醒");

    // 2. 子代理终态同样跨回合唤醒：否则结果只能等下一位用户输入才被装配，
    //    而提示词宣称「子代理跑完会自动通知你」。
    publish_completion_notice(CompletionNotice::new(
        JobProducerKind::Subagent,
        format!("subagent:ended:{}", uuid::Uuid::new_v4()),
        "epoch:9".to_owned(),
        owner.clone(),
        JobTerminalKind::Completed,
        "正常结束",
        "done",
        Some("/tmp/tee/job-9.log".to_owned()),
        Some("/root/review".to_owned()),
    ));
    assert_eq!(fixture.wake_count(), 1, "子代理终态必须唤醒新回合");

    // 3. 重新运行一轮未取消的新回合结束后，用户手动停止的作业应正常唤醒。
    fixture.bridge.register_turn(
        &owner,
        WakeRouteInfo {
            session_id: fixture.session_id.clone(),
            model: None,
            reasoning_effort: None,
            turn_cancel_token: RunCancelToken::default(),
        },
    );
    publish_completion_notice(shell_notice(
        &owner,
        &format!("shell:user-stop:{}", uuid::Uuid::new_v4()),
        JobTerminalKind::Killed,
        JOB_REASON_USER_STOP,
    ));
    assert_eq!(fixture.wake_count(), 2);
}

#[test]
fn multi_notice_batch_respects_interaction_id_length_and_ignores_system_channel() {
    // 多条长 notice_key 合并成一批时，InteractionId 与 source_message_id 仍必须满足长度上界。
    let fixture = TestFixture::new("desktop", "workbench-multi", "ws-multi");
    let owner = format!("run-multi-{}", uuid::Uuid::new_v4());
    fixture.bridge.register_turn(
        &owner,
        WakeRouteInfo {
            session_id: fixture.session_id.clone(),
            model: None,
            reasoning_effort: None,
            turn_cancel_token: RunCancelToken::default(),
        },
    );
    let batch = (0..5)
        .map(|index| {
            shell_notice(
                &owner,
                &format!("shell:{}:{index}:completed", uuid::Uuid::new_v4()),
                JobTerminalKind::Completed,
                "正常结束",
            )
        })
        .collect::<Vec<_>>();
    publish_completion_notices(batch);
    assert_eq!(fixture.wake_count(), 1);
    assert_eq!(fixture.session_interactions().len(), 2);

    // system 渠道不得触发唤醒。
    let system_fixture = TestFixture::new(SYSTEM_SOURCE_CHANNEL, "internal", "ws-sys");
    let sys_owner = format!("run-sys-{}", uuid::Uuid::new_v4());
    system_fixture.bridge.register_turn(
        &sys_owner,
        WakeRouteInfo {
            session_id: system_fixture.session_id.clone(),
            model: None,
            reasoning_effort: None,
            turn_cancel_token: RunCancelToken::default(),
        },
    );
    publish_completion_notice(shell_notice(
        &sys_owner,
        &format!("shell:sys:{}", uuid::Uuid::new_v4()),
        JobTerminalKind::Completed,
        "正常结束",
    ));
    assert_eq!(system_fixture.wake_count(), 0);
}

#[test]
fn releasing_an_idle_turn_drops_the_route_and_its_subscription() {
    let fixture = TestFixture::new("desktop", "workbench-release", "ws-release");
    let owner = format!("run-release-{}", uuid::Uuid::new_v4());
    fixture.bridge.register_turn(
        &owner,
        WakeRouteInfo {
            session_id: fixture.session_id.clone(),
            model: None,
            reasoning_effort: None,
            turn_cancel_token: RunCancelToken::default(),
        },
    );

    // 回合收口且该 owner 名下已无未终态作业：路由与随行通知订阅都必须被摘除。
    // 路由表不回收时，每派生一个子代理就新增一个 owner，订阅会随运行时长单调增长。
    fixture.bridge.release_idle_turn(&owner);
    publish_completion_notice(shell_notice(
        &owner,
        &format!("shell:released:{}", uuid::Uuid::new_v4()),
        JobTerminalKind::Completed,
        "正常结束",
    ));
    assert_eq!(fixture.wake_count(), 0, "已注销的 owner 不得再唤醒新回合");
    let wake_interactions = fixture
        .session_interactions()
        .into_iter()
        .filter(|interaction| {
            interaction
                .source
                .source_message_id()
                .starts_with(SHELL_JOB_WAKE_SOURCE_PREFIX)
        })
        .count();
    assert_eq!(
        wake_interactions, 0,
        "已注销的 owner 不得再提交唤醒 Interaction"
    );

    // 未注销的 owner 仍照常唤醒：证明上面的静默来自注销本身，而不是别的抑制条件。
    let live_owner = format!("run-live-{}", uuid::Uuid::new_v4());
    fixture.bridge.register_turn(
        &live_owner,
        WakeRouteInfo {
            session_id: fixture.session_id.clone(),
            model: None,
            reasoning_effort: None,
            turn_cancel_token: RunCancelToken::default(),
        },
    );
    publish_completion_notice(shell_notice(
        &live_owner,
        &format!("shell:live:{}", uuid::Uuid::new_v4()),
        JobTerminalKind::Completed,
        "正常结束",
    ));
    assert_eq!(fixture.wake_count(), 1, "未注销的 owner 必须照常唤醒");
}
