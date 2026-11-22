//! supervisor tick 的静默轮次收口验收（WP-04 服务端一半）。
//!
//! 固定两件事：期限未到时不收口、期限一过必须在既有维护 tick 里收口为 interrupted 并
//! 写 `terminal_kind`；同时锁定「收口来自静默路径而不是过期租约隔离路径」。

use super::*;
use crate::application::silent_turn_settlement::DEFAULT_FIRST_TOKEN_DEADLINE_MS;
use axon_domain::{
    AudienceContext, AudienceKind, ConsumerCursor, DisclosurePolicy, InteractionRecord,
    InteractionSource, ReplyRoute, ScopeRef,
};
use axon_session::{
    InteractionQueryStore, InteractionStore, RuntimeStore, SessionEventRecord, SqliteRuntimeStore,
    conclusive_work_items_without_terminal_kind_sql,
};
use axon_session_runtime::{ActionExecutionRequest, TurnExecutionError, TurnExecutionOutcome};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const TICK_BASE_NOW_MS: u64 = 3_000_000;
const TICK_HOST_LEASE_MS: u64 = 300_000;
/// 轮次租约远长于判死期限：这一轮是「有人持有租约但 provider 静默」，不是租约过期。
const TICK_TURN_LEASE_MS: u64 = 10_000_000;
/// 领取时刻相对基准时钟的偏移：判死期限从领取起算，不是从基准时钟起算。
const TICK_CLAIM_OFFSET_MS: u64 = 10;

#[derive(Clone)]
struct TickClock(Arc<AtomicU64>);

impl TickClock {
    fn new(now_ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(now_ms)))
    }

    fn advance_by(&self, delta_ms: u64) {
        self.0.fetch_add(delta_ms, Ordering::AcqRel);
    }
}

impl RuntimeClock for TickClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

/// tick 用例不执行任何轮次：判死只依赖库里的租约与活动事实。
#[derive(Clone, Copy)]
struct IdleExecutor;

impl TurnExecutor for IdleExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Err(TurnExecutionError::permanent("tick 用例不执行轮次"))
    }
}

#[derive(Clone, Copy)]
struct TickPublisher;

impl EventPublisher for TickPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct TickProcess;

impl ProcessIdentity for TickProcess {
    fn pid(&self) -> u32 {
        1
    }

    fn process_start_id(&self) -> Option<String> {
        None
    }
}

#[test]
fn silent_turn_settlement_runs_on_supervisor_tick() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let clock = TickClock::new(TICK_BASE_NOW_MS);
    let mut host = DaemonHost::new(
        store.clone(),
        IdleExecutor,
        TickPublisher,
        clock.clone(),
        TickProcess,
        RuntimeHostId::new("daemon-silent-turn-host").expect("Host ID"),
        TICK_HOST_LEASE_MS,
        TICK_HOST_LEASE_MS,
    );
    host.start().expect("启动 Host");

    let session_id = DaemonSessionId::new("session-silent-tick").expect("Session ID");
    store
        .create_session(&session_id, ".", TICK_BASE_NOW_MS)
        .expect("建立会话");
    let interaction = interaction_record(&session_id, "interaction-silent-tick");
    store.append(&interaction).expect("写入入站事实");
    // 领取之后一个事件帧都没有：租约仍在有效期内，因此不会被过期租约隔离路径碰到。
    store
        .claim_processing_by_id(
            &interaction.interaction_id,
            TICK_BASE_NOW_MS + TICK_CLAIM_OFFSET_MS,
            TICK_TURN_LEASE_MS,
        )
        .expect("领取")
        .expect("租约");

    // 首字期限未到：这一轮还在跑，tick 不得动它。
    let early = host
        .supervisor_tick(DEFAULT_QUEUE_LIMIT)
        .expect("首轮 tick");
    assert_eq!(early.silent_turns_settled, 0, "期限未到时不得收口");
    assert_eq!(
        turn_status(&store, &session_id, interaction.interaction_id.as_str()),
        InteractionStatus::Processing,
        "期限未到时轮次必须保持在跑"
    );

    // 期限已过（从领取时刻起算）：同一套维护 tick 必须把它收口为 interrupted。
    clock.advance_by(DEFAULT_FIRST_TOKEN_DEADLINE_MS + TICK_CLAIM_OFFSET_MS);
    let late = host
        .supervisor_tick(DEFAULT_QUEUE_LIMIT)
        .expect("期限后的 tick");
    assert_eq!(
        late.interrupted_interactions, 0,
        "租约仍有效：收口必须来自静默路径，而不是过期租约隔离"
    );
    assert_eq!(
        late.silent_turns_settled, 1,
        "首字超时的轮次必须在 supervisor tick 里收口"
    );
    assert_eq!(
        turn_status(&store, &session_id, interaction.interaction_id.as_str()),
        InteractionStatus::Interrupted,
        "收口后索引必须读得出终态"
    );
    assert_eq!(
        terminal_kind(&store, interaction.interaction_id.as_str()).as_deref(),
        Some("interrupted"),
        "终态必须同时写 terminal_kind（INV-2）"
    );
    assert_eq!(conclusion_violations(&store), 0, "INV-2 必须为零行");

    // 下一轮 tick 不得重复收口：幂等由存储层的状态守卫保证。
    let again = host
        .supervisor_tick(DEFAULT_QUEUE_LIMIT)
        .expect("重复 tick");
    assert_eq!(again.silent_turns_settled, 0);
}

fn interaction_record(session_id: &DaemonSessionId, interaction_id: &str) -> InteractionRecord {
    InteractionRecord::new(
        session_id.clone(),
        InteractionId::new(interaction_id).expect("Interaction ID"),
        InteractionSource::new("tui", format!("message-{interaction_id}")).expect("来源"),
        ScopeRef::workspace("workspace-test").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", "terminal-test").expect("路由"),
        "检查静默轮次",
        TICK_BASE_NOW_MS - 2_000,
    )
    .expect("Interaction")
}

fn turn_status(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    interaction_id: &str,
) -> InteractionStatus {
    store
        .interaction_turn_heads(session_id.as_str(), 10)
        .expect("轮次头")
        .into_iter()
        .find(|head| head.interaction_id == interaction_id)
        .map_or(InteractionStatus::Received, |head| head.status)
}

fn terminal_kind(store: &SqliteRuntimeStore, interaction_id: &str) -> Option<String> {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT terminal_kind FROM work_items_all WHERE inbox_id=?1",
                [interaction_id],
                |row| row.get::<_, Option<String>>(0),
            )
        })
        .ok()
        .flatten()
}

fn conclusion_violations(store: &SqliteRuntimeStore) -> i64 {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                &conclusive_work_items_without_terminal_kind_sql(),
                [],
                |row| row.get(0),
            )
        })
        .expect("INV-2 检查")
}
