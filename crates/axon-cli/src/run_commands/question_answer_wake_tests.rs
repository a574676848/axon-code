//! 迟到作答唤醒链路的真实装配测试。
//!
//! 覆盖的三条语义都落在「谁开启了这一轮」上，因此全部用真实零件：真实答案服务、
//! 真实车道答案注入器、真实 SQLite 运行态仓库、真实主 turn 领取服务。模型侧换成
//! 记录型 runner（记录每一轮吃到的 Interaction），机制本身不被替换。

use std::sync::{Arc, Mutex};

use axon_agent_runtime::AgentTurnSteerSource;
use axon_daemon::{
    PrimaryTurnRequest, PrimaryTurnRunner, PrimaryTurnRunnerError, PrimaryTurnService,
};
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, InteractionStatus, ReplyRoute, ScopeRef, UserAnswerItem, UserQuestionRecord,
    UserQuestionState,
};
use axon_session::{
    InteractionQueryStore, InteractionStore, SqliteRuntimeStore, UserQuestionStore,
};
use axon_session_runtime::RuntimeClock;

use super::RunCancelToken;
use super::run_steer::{ActivePrimaryTurnRegistry, RunSteerBridge};
use crate::user_question::QuestionMessageSteerer;
use crate::user_question::lane_steerer::PrimaryTurnWakeHook;
use crate::user_question::{
    LaneQuestionMessageSteerer, NoopUserQuestionEventSink, UserQuestionAnswerService,
};

/// 测试车道所属的工作区。
const WORKSPACE_ID: &str = "workspace-late-answer";
/// 测试车道的会话表面。
const LANE_SURFACE: &str = "desktop-surface-1";
/// 测试车道来源渠道；必须与回复路由渠道一致，否则入口策略直接拒绝。
const LANE_CHANNEL: &str = "desktop";
/// 动作租约时长；与守护进程同一量级即可。
const TEST_LEASE_MS: u64 = 120_000;
/// 单次领取批量上限。
const TICK_BATCH: u32 = 32;

#[test]
fn late_answer_lands_durable_and_opens_exactly_one_new_primary_turn() {
    let fixture = WakeFixture::new();
    let record = fixture.pending_question("call-late-answer");
    let runner = RecordingRunner::default();

    let settled = fixture
        .answer_service()
        .submit_answer(
            &record.lane_id,
            &record.tool_call_id,
            answer("release"),
            "desktop",
        )
        .expect("挂起提问的迟到作答必须被接受");

    // 没有活等待者时答案只能走消息通道，且它必须真的落成 durable Interaction。
    assert_eq!(
        settled.delivery_mode.as_deref(),
        Some("user_message"),
        "没有活等待者时答案必须走消息通道"
    );
    let queued = fixture
        .queued_answer(&settled.question_id)
        .expect("答案必须落成 durable Interaction");
    assert_eq!(
        queued.status,
        InteractionStatus::Received,
        "答案落库时是待领取的排队态"
    );
    assert_eq!(
        fixture.wake_count(),
        1,
        "答案落库后必须显式唤醒一次主 turn 领取"
    );

    // 唤醒之后由真实领取服务接管：它必须真的开启一轮，且这一轮吃到的就是这条答案。
    let tick = fixture.tick(&runner);
    assert_eq!(tick.claimed, 1, "唤醒后必须领取到这条答案");
    let answer_interaction = answer_interaction_id(&settled.question_id);
    assert_eq!(
        runner.interaction_ids(),
        vec![answer_interaction.as_str().to_owned()],
        "新的一轮必须由答案 Interaction 开启"
    );
    assert!(
        runner.contents()[0].contains(&record.tool_call_id),
        "这一轮的正文必须是答案回执: {}",
        runner.contents()[0]
    );
    assert_eq!(
        fixture
            .queued_answer(&settled.question_id)
            .expect("答案仍在仓库里")
            .status,
        InteractionStatus::Understood,
        "领取完成后答案必须收敛为已理解"
    );
}

#[test]
fn duplicate_answer_delivery_never_opens_a_second_turn() {
    let fixture = WakeFixture::new();
    let record = fixture.pending_question("call-duplicate-answer");
    let service = fixture.answer_service();
    let runner = RecordingRunner::default();

    let settled = service
        .submit_answer(
            &record.lane_id,
            &record.tool_call_id,
            answer("release"),
            "desktop",
        )
        .expect("首次作答必须被接受");
    assert_eq!(fixture.tick(&runner).claimed, 1, "首次作答开启一轮");

    // 重试投递：权威记录已结算，统一入口直接拒绝，不会开出第二轮。
    let retry = service.submit_answer(
        &record.lane_id,
        &record.tool_call_id,
        answer("release"),
        "desktop",
    );
    assert!(
        matches!(
            retry,
            Err(crate::user_question::AnswerError::AlreadySettled(_))
        ),
        "重复投递必须被权威记录挡下: {retry:?}"
    );

    // 对账补投路径同样不得重投：送达事实已经落库。
    assert_eq!(
        service.reconcile_undelivered(10).expect("对账"),
        0,
        "已送达的作答不再进入补投集合"
    );

    // 即使有人绕过权威记录按同一幂等键重投，稳定 Interaction ID 也让重复投递命中同
    // 一条记录（重试回放的是同一条正文），不会被领取成第二轮。
    let steerer = fixture.steerer();
    let answer_content = fixture
        .queued_answer(&settled.question_id)
        .expect("答案必须落成 durable Interaction")
        .content;
    for _ in 0..2 {
        steerer
            .steer_answer_message(
                &record.lane_id,
                &record.session_id,
                &format!("question-answer-{}", settled.question_id),
                &answer_content,
            )
            .expect("重复投递必须被稳定幂等键吸收");
    }
    assert_eq!(
        fixture.answer_interaction_count(),
        1,
        "同一条答案只有一条记录"
    );
    assert_eq!(fixture.tick(&runner).claimed, 0, "重复投递不得再开启一轮");
    assert_eq!(runner.round_count(), 1, "整段流程只允许一轮");
}

#[test]
fn answer_merges_into_the_active_turn_instead_of_opening_a_new_one() {
    let fixture = WakeFixture::new();
    let record = fixture.pending_question("call-active-turn");
    let runner = RecordingRunner::default();

    // 车道上确实有一轮正在运行的主 turn：真实注册表 + 真实注入邮箱。
    let bridge = RunSteerBridge::default();
    let inbox = bridge.open();
    let drain_handle = inbox.clone();
    let _live_turn = fixture
        .active_primary_turns
        .register(
            &ActivePrimaryTurnRegistry::affinity_key(&record.lane_id),
            InteractionId::new("interaction-active-turn").expect("合法 ID"),
            inbox,
            RunCancelToken::default(),
        )
        .expect("车道空闲时必须能登记活跃主 turn");

    let settled = fixture
        .answer_service()
        .submit_answer(
            &record.lane_id,
            &record.tool_call_id,
            answer("release"),
            "desktop",
        )
        .expect("有活跃主 turn 时作答同样必须被接受");

    assert_eq!(
        settled.delivery_mode.as_deref(),
        Some("user_message"),
        "没有活等待者时投递方式仍是消息通道"
    );
    // 答案并入那一轮：不落新的 durable Interaction，也不唤醒开新轮。
    assert!(
        fixture.queued_answer(&settled.question_id).is_none(),
        "有活跃主 turn 时答案不得另落一条待领取记录"
    );
    assert_eq!(fixture.wake_count(), 0, "有活跃主 turn 时不得唤醒开新轮");

    // 并入的证据是内容进了活跃那一轮的注入邮箱。
    let steered = drain_handle.drain();
    assert_eq!(steered.len(), 1, "答案必须并入正在运行的那一轮");
    assert!(
        steered[0].message.content.contains(&record.tool_call_id),
        "并入的正文必须是答案回执: {}",
        steered[0].message.content
    );

    assert_eq!(
        fixture.tick(&runner).claimed,
        0,
        "并入活跃轮之后不得再开启新的一轮"
    );
    assert_eq!(runner.round_count(), 0);
}

/// 一次作答：单选一个问题。
fn answer(selected: &str) -> Vec<UserAnswerItem> {
    vec![UserAnswerItem {
        id: "q1".to_owned(),
        selected: vec![selected.to_owned()],
        custom: None,
    }]
}

/// 记录每一轮主 turn 的 runner；只替换模型侧，不替换领取与投递机制。
#[derive(Clone, Default)]
struct RecordingRunner {
    rounds: Arc<Mutex<Vec<PrimaryTurnRequest>>>,
}

impl RecordingRunner {
    fn round_count(&self) -> usize {
        self.rounds.lock().expect("锁未中毒").len()
    }

    fn interaction_ids(&self) -> Vec<String> {
        self.rounds
            .lock()
            .expect("锁未中毒")
            .iter()
            .map(|request| request.interaction.interaction_id.as_str().to_owned())
            .collect()
    }

    fn contents(&self) -> Vec<String> {
        self.rounds
            .lock()
            .expect("锁未中毒")
            .iter()
            .map(|request| request.interaction.content.clone())
            .collect()
    }
}

impl PrimaryTurnRunner for RecordingRunner {
    fn run(&self, request: PrimaryTurnRequest) -> Result<(), PrimaryTurnRunnerError> {
        self.rounds.lock().expect("锁未中毒").push(request);
        Ok(())
    }
}

/// 运行时钟：用真实系统时间，避免测试对时间做假设。
#[derive(Clone, Copy)]
struct TestClock;

impl RuntimeClock for TestClock {
    fn now_ms(&self) -> u64 {
        axon_time::unix_now_ms()
    }
}

/// 迟到作答的现场：一条已登记的车道、一份答案服务装配与唤醒计数。
struct WakeFixture {
    store: SqliteRuntimeStore,
    active_primary_turns: ActivePrimaryTurnRegistry,
    wakes: Arc<Mutex<usize>>,
    lane_id: String,
}

impl WakeFixture {
    /// 建立车道并把种子记录结算掉，后续断言只观察本次作答产生的记录。
    fn new() -> Self {
        let store = SqliteRuntimeStore::from_root(axon_test_support::test_temp_root());
        let active_primary_turns = ActivePrimaryTurnRegistry::default();
        let seed = InteractionRecord::new(
            InteractionId::new("interaction-lane-seed").expect("合法 ID"),
            InteractionSource::new(LANE_CHANNEL, "seed-message-1").expect("合法来源"),
            ScopeRef::workspace(WORKSPACE_ID).expect("合法作用域"),
            private_audience(),
            ReplyRoute::new(LANE_CHANNEL, LANE_SURFACE).expect("合法回复路由"),
            "历史提问",
            1,
        )
        .expect("合法 Interaction");
        let lane_id =
            axon_session::interaction_session_lane_id(&seed).expect("种子记录必须绑定 Workspace");
        store.append(&seed).expect("种子 Interaction 落库");
        let fixture = Self {
            store,
            active_primary_turns,
            wakes: Arc::new(Mutex::new(0)),
            lane_id,
        };
        // 种子记录必须先被结算，否则它自己就会在后续领取里占掉一轮。
        let seeded = fixture.tick(&RecordingRunner::default());
        assert_eq!(seeded.claimed, 1, "种子记录应被领取一次");
        fixture
    }

    /// 待决提问记录；车道必须与已登记车道一致，否则答案入口按跨车道拒绝。
    fn pending_question(&self, call_id: &str) -> UserQuestionRecord {
        let record = UserQuestionRecord {
            question_id: format!("uq-{call_id}"),
            idempotency_key: format!("idem-{call_id}"),
            lane_id: self.lane_id.clone(),
            session_id: self.lane_id.clone(),
            interaction_id: "interaction-question".to_owned(),
            turn_id: "turn-question".to_owned(),
            tool_call_id: call_id.to_owned(),
            questions_json:
                r#"[{"id":"q1","question":"本次发布走哪个分支？","options":[{"label":"release"}],"multi_select":false}]"#
                    .to_owned(),
            handoff_text: Some("任务已暂时终止，等待用户作答".to_owned()),
            state: UserQuestionState::Suspended,
            deadline_at_ms: 10_000,
            answer_json: None,
            answered_at_ms: None,
            answered_channel: None,
            delivered: false,
            delivery_mode: None,
            created_at_ms: 1_000,
            expires_at_ms: 86_400_000,
        };
        self.store.insert_question(&record).expect("提问记录落库");
        record
    }

    /// 车道答案注入器：与守护进程同一份装配，唤醒端口计入本次现场。
    fn steerer(&self) -> LaneQuestionMessageSteerer {
        LaneQuestionMessageSteerer::new(
            self.store.clone(),
            self.active_primary_turns.clone(),
            TEST_LEASE_MS,
            self.wake_hook(),
        )
    }

    /// 答案服务：没有活等待者，因此迟到作答只能走消息通道。
    fn answer_service(&self) -> UserQuestionAnswerService {
        UserQuestionAnswerService::new(
            Arc::new(self.store.clone()),
            None,
            Arc::new(NoopUserQuestionEventSink),
            Arc::new(self.steerer()),
        )
    }

    /// 运行一轮真实主 turn 领取。
    fn tick(&self, runner: &RecordingRunner) -> axon_daemon::PrimaryTurnTickReport {
        PrimaryTurnService::new(self.store.clone(), runner.clone(), TestClock, TEST_LEASE_MS)
            .expect("主 turn 服务装配")
            .tick(TICK_BATCH)
            .expect("主 turn 领取")
    }

    /// 该次答案的 durable Interaction；未落库时返回 None。
    fn queued_answer(&self, question_id: &str) -> Option<InteractionRecord> {
        self.store
            .get_interaction(&answer_interaction_id(question_id))
            .expect("读取答案 Interaction")
    }

    /// 该车道上的答案 Interaction 条数。
    fn answer_interaction_count(&self) -> usize {
        self.store
            .interactions_for_lane(&self.lane_id, 10, None)
            .expect("读取车道 Interaction")
            .iter()
            .filter(|record| {
                record
                    .interaction_id
                    .as_str()
                    .starts_with(ANSWER_INTERACTION_PREFIX)
            })
            .count()
    }

    fn wake_count(&self) -> usize {
        *self.wakes.lock().expect("锁未中毒")
    }

    fn wake_hook(&self) -> PrimaryTurnWakeHook {
        let wakes = Arc::clone(&self.wakes);
        Arc::new(move || {
            *wakes.lock().expect("锁未中毒") += 1;
            Ok(())
        })
    }
}

/// 答案 Interaction 的稳定前缀；与答案注入器的幂等键口径一致。
const ANSWER_INTERACTION_PREFIX: &str = "question-answer-";

fn answer_interaction_id(question_id: &str) -> InteractionId {
    InteractionId::new(format!("{ANSWER_INTERACTION_PREFIX}{question_id}")).expect("合法答案 ID")
}

fn private_audience() -> AudienceContext {
    AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly)
}
