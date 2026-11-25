//! 运行态轮次事实的等价性测试：同一份夹具下新读法与改造前的两条查询一致。
//!
//! `/live` 的两个数字直接决定界面说不说「在跑」、排不排「已提交未领取」，因此口径必须
//! 与原实现逐项对齐：在跑行取租约最大值、排队行要求派发轮尚未收口、没有入站事实的
//! 工作项（目标执行）不算，别的会话的轮次也不能串进来。

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};
use axon_session::{
    InteractionSettlementStore as _, InteractionStore as _, RuntimeStore as _, SqliteRuntimeStore,
};

use super::{LiveTurnFacts, LiveTurnFactsStore};

/// 固定判定时刻；租约是否过期由各用例自己安排。
const NOW_MS: u64 = 1_760_000_000_000;

/// 初始化一个空的运行态库。
fn fixture() -> (axon_test_support::TestTempDir, SqliteRuntimeStore) {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(directory.path().join("data"));
    store.initialize().expect("初始化运行态");
    (directory, store)
}

/// 登记一条会话。
fn session(store: &SqliteRuntimeStore, raw: &str) -> DaemonSessionId {
    let session_id = DaemonSessionId::new(raw).expect("会话标识");
    store
        .create_session(&session_id, "/workspace", NOW_MS)
        .expect("登记会话");
    session_id
}

/// 造一条入站事实。
fn interaction(session_id: &DaemonSessionId, interaction_id: &str) -> InteractionRecord {
    InteractionRecord::new(
        session_id.clone(),
        InteractionId::new(interaction_id).expect("交互标识"),
        InteractionSource::new("tui", format!("message-{interaction_id}")).expect("来源"),
        ScopeRef::workspace("workspace-test").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("desktop", "window-1").expect("回复路由"),
        "这一轮",
        NOW_MS,
    )
    .expect("入站事实")
}

/// 直接写一条工作项：`inbox_id` 为空表示它不挂在入站事实上。
fn seed_work_item(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    execution_id: &str,
    inbox_id: Option<&str>,
    kind: &str,
    status: &str,
    lease_expires_at_ms: Option<i64>,
) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO work_items(execution_id,session_id,kind,payload_json,attempt,priority,
                     inbox_id,status,lease_expires_at_ms,created_at_ms,updated_at_ms)
                     VALUES(?1,?2,?3,'{}',1,0,?4,?5,?6,1,1)",
                    rusqlite::params![
                        execution_id,
                        session_id.as_str(),
                        kind,
                        inbox_id,
                        status,
                        lease_expires_at_ms
                    ],
                )
                .map(|_| ())
        })
        .expect("写入工作项");
}

/// 把一条派发行的终态写上：排队行只有在派发轮未收口时才算排队。
fn settle_dispatch_inbox(store: &SqliteRuntimeStore, interaction_id: &str) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "UPDATE inbox SET terminal_kind='understood' WHERE interaction_id=?1",
                    rusqlite::params![interaction_id],
                )
                .map(|_| ())
        })
        .expect("结算派发轮");
}

/// 改造前的两条查询：在跑轮次的租约集合与排队计数。
fn legacy_facts(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> (Option<u64>, u64) {
    let turns = store.in_flight_turns(session_id).expect("读取在跑轮次");
    let queued = store.queued_turn_count(session_id).expect("读取排队轮次");
    (
        turns
            .iter()
            .filter_map(|turn| turn.lease_expires_at_ms)
            .max(),
        queued,
    )
}

fn assert_same(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    label: &str,
) -> LiveTurnFacts {
    let (legacy_lease, legacy_queued) = legacy_facts(store, session_id);
    let facts = store.live_turn_facts(session_id).expect("读取轮次事实");
    assert_eq!(
        facts.running_lease_expires_at_ms, legacy_lease,
        "{label}：在跑轮次的租约上界必须与改造前一致"
    );
    assert_eq!(
        facts.queued_turns, legacy_queued,
        "{label}：排队轮次数必须与改造前一致"
    );
    facts
}

#[test]
fn live_facts_match_the_inbox_driven_queries() {
    let (_directory, store) = fixture();

    // 一、没有任何轮次：两个数都是空与零。
    let idle = session(&store, "session-facts-idle");
    assert_eq!(
        assert_same(&store, &idle, "空会话"),
        LiveTurnFacts::default()
    );

    // 二、一条租约未过期的在跑轮次：走真实领取路径，工作项由存储层建。
    let fresh = session(&store, "session-facts-fresh");
    let fresh_turn = interaction(&fresh, "interaction-facts-fresh");
    store.append(&fresh_turn).expect("写入入站事实");
    store
        .claim_processing_by_id(&fresh_turn.interaction_id, NOW_MS, 60_000)
        .expect("领取")
        .expect("租约");
    let facts = assert_same(&store, &fresh, "单条在跑");
    assert_eq!(facts.running_lease_expires_at_ms, Some(NOW_MS + 60_000));
    assert_eq!(facts.queued_turns, 0);

    // 三、两条在跑轮次：取最晚的租约。
    let multi = session(&store, "session-facts-multi");
    let first_turn = interaction(&multi, "interaction-facts-multi-1");
    let second_turn = interaction(&multi, "interaction-facts-multi-2");
    store.append(&first_turn).expect("写入第一条");
    store.append(&second_turn).expect("写入第二条");
    store
        .claim_processing_by_id(&first_turn.interaction_id, NOW_MS, 1_000)
        .expect("领取第一条")
        .expect("租约");
    store
        .claim_processing_by_id(&second_turn.interaction_id, NOW_MS, 9_000)
        .expect("领取第二条")
        .expect("租约");
    assert_eq!(
        assert_same(&store, &multi, "多条在跑").running_lease_expires_at_ms,
        Some(NOW_MS + 9_000),
        "在跑只报最晚的那条租约"
    );

    // 四、租约已过期：事实层照实给出，过不过期由调用方按当前时刻判定。
    let stale = session(&store, "session-facts-stale");
    let stale_turn = interaction(&stale, "interaction-facts-stale");
    store.append(&stale_turn).expect("写入入站事实");
    store
        .claim_processing_by_id(&stale_turn.interaction_id, NOW_MS - 1_000, 100)
        .expect("领取")
        .expect("租约");
    assert_eq!(
        assert_same(&store, &stale, "过期租约").running_lease_expires_at_ms,
        Some(NOW_MS - 900)
    );

    // 五、释放回排队的轮次：它是活的，但没有租约。
    let queued = session(&store, "session-facts-queued");
    let queued_turn = interaction(&queued, "interaction-facts-queued");
    store.append(&queued_turn).expect("写入入站事实");
    let lease = store
        .claim_processing_by_id(&queued_turn.interaction_id, NOW_MS, 60_000)
        .expect("领取")
        .expect("租约");
    store
        .release_processing(&queued_turn.interaction_id, &lease.lease_token, NOW_MS)
        .expect("释放回排队");
    let facts = assert_same(&store, &queued, "排队轮次");
    assert_eq!(facts.queued_turns, 1, "释放回排队的轮次必须计入");
    assert_eq!(facts.running_lease_expires_at_ms, None, "排队态没有租约");
}

#[test]
fn work_items_without_live_inbox_rows_do_not_count() {
    let (_directory, store) = fixture();
    let lane = session(&store, "session-facts-lane");
    let other = session(&store, "session-facts-other");

    // 一、目标执行的工作项：会话标识在，但没有入站事实，不算任何一类轮次。
    seed_work_item(
        &store,
        &lane,
        "exec-goal-running",
        None,
        "goal",
        "running",
        Some(1),
    );
    seed_work_item(
        &store,
        &lane,
        "exec-goal-queued",
        None,
        "goal",
        "queued",
        None,
    );
    assert_eq!(
        assert_same(&store, &lane, "只有目标执行"),
        LiveTurnFacts::default(),
        "没有入站事实的工作项不计入在跑与排队"
    );

    // 二、派发轮已收口的排队行：原口径要求 `inbox.terminal_kind IS NULL`。
    let dispatch_turn = interaction(&lane, "interaction-facts-dispatch");
    store.append(&dispatch_turn).expect("写入派发轮");
    seed_work_item(
        &store,
        &lane,
        "exec-dispatch-queued",
        Some(dispatch_turn.interaction_id.as_str()),
        "turn",
        "queued",
        None,
    );
    assert_eq!(
        assert_same(&store, &lane, "派发轮未收口").queued_turns,
        1,
        "派发轮未收口时排队行计入"
    );
    settle_dispatch_inbox(&store, dispatch_turn.interaction_id.as_str());
    assert_eq!(
        assert_same(&store, &lane, "派发轮已收口").queued_turns,
        0,
        "派发轮收口后的排队行不再计入"
    );

    // 三、别的会话的在跑轮次不能串进这一条。
    let other_turn = interaction(&other, "interaction-facts-other");
    store.append(&other_turn).expect("写入别的会话的入站事实");
    seed_work_item(
        &store,
        &other,
        "exec-other-running",
        Some(other_turn.interaction_id.as_str()),
        "turn",
        "running",
        Some(NOW_MS.saturating_add(5_000) as i64),
    );
    let (legacy_lease, legacy_queued) = legacy_facts(&store, &lane);
    let facts = store.live_turn_facts(&lane).expect("读取轮次事实");
    assert_eq!(
        facts.running_lease_expires_at_ms, None,
        "别的会话的轮次不算本会话在跑"
    );
    assert_eq!(facts.queued_turns, 0);
    assert_eq!(legacy_lease, None, "基准同样不计");
    assert_eq!(legacy_queued, 0, "基准同样不计");
}

/// 在跑行没有租约：两侧都读作「没有在跑」，不补零。
#[test]
fn running_without_lease_reports_no_lease() {
    let (_directory, store) = fixture();
    let lane = session(&store, "session-facts-no-lease");
    let turn = interaction(&lane, "interaction-facts-no-lease");
    store.append(&turn).expect("写入入站事实");
    seed_work_item(
        &store,
        &lane,
        "exec-no-lease",
        Some(turn.interaction_id.as_str()),
        "turn",
        "running",
        None,
    );
    let (legacy_lease, legacy_queued) = legacy_facts(&store, &lane);
    let facts = store.live_turn_facts(&lane).expect("读取轮次事实");
    assert_eq!(legacy_lease, None, "基准里没有租约");
    assert_eq!(
        facts.running_lease_expires_at_ms, None,
        "新读法同样为空，不补零"
    );
    assert_eq!(facts.queued_turns, legacy_queued);
}
