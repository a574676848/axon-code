//! `recovering` 收敛测试：这个状态必须有归宿（INV-7）。
//!
//! 三条用例各钉住一件事：
//! - 没有在途工作项的会话必须回 `idle`，不能停在 `recovering`；
//! - 超过 TTL 仍未派发的工作项必须收口为 `interrupted` 并留下事件；
//! - 一轮收敛之后，除「仍有有效租约在跑」之外不允许再剩 `recovering`。

use super::SqliteRuntimeStore;
use crate::ports::{EventLogStore as _, InteractionStore as _, RuntimeStore};
use axon_domain::{DaemonSessionId, RuntimeHostId, SessionRuntimeStatus};
use rusqlite::params;

/// 收敛 TTL：与 daemon 侧生产取值的量级一致，只用于把「未到期」与「已到期」分开。
const TTL_MS: u64 = 120_000;

fn store_at(tag: &str) -> (std::path::PathBuf, SqliteRuntimeStore) {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join(format!("recovering-{tag}.sqlite3"));
    (path.clone(), SqliteRuntimeStore::new(path))
}

fn host(store: &SqliteRuntimeStore, name: &str) -> crate::domain::runtime::DaemonHostRecord {
    store
        .claim_host(
            &RuntimeHostId::new(name).expect("host"),
            1,
            None,
            10,
            1_000_000,
        )
        .expect("host lease")
}

fn session(store: &SqliteRuntimeStore, name: &str) -> DaemonSessionId {
    let session_id = DaemonSessionId::new(name).expect("session");
    store.create_session(&session_id, ".", 10).expect("session");
    session_id
}

/// 崩溃现场：会话已经被写成 `recovering`，活动时间停在这一刻。
fn mark_recovering(store: &SqliteRuntimeStore, session_id: &DaemonSessionId, now_ms: i64) {
    store
        .connection()
        .expect("connection")
        .execute(
            "UPDATE sessions SET status='recovering',last_activity_at_ms=?1,updated_at_ms=?1 \
             WHERE session_id=?2",
            params![now_ms, session_id.as_str()],
        )
        .expect("写 recovering");
}

/// 在途工作项：`status` 决定它是「从未派发」还是「有人在跑」。
fn insert_work_item(
    store: &SqliteRuntimeStore,
    execution_id: &str,
    session_id: &DaemonSessionId,
    inbox_id: Option<&str>,
    status: &str,
    lease_expires_at_ms: Option<i64>,
    created_at_ms: i64,
) {
    store
        .connection()
        .expect("connection")
        .execute(
            "INSERT INTO work_items(execution_id,goal_id,session_id,kind,payload_json,attempt,\
             priority,status,inbox_id,lease_expires_at_ms,created_at_ms,updated_at_ms) \
             VALUES(?1,NULL,?2,'turn','{}',0,0,?3,?4,?5,?6,?6)",
            params![
                execution_id,
                session_id.as_str(),
                status,
                inbox_id,
                lease_expires_at_ms,
                created_at_ms
            ],
        )
        .expect("写在途工作项");
}

fn session_status(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> String {
    store
        .connection()
        .expect("connection")
        .query_row(
            "SELECT status FROM sessions WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get(0),
        )
        .expect("读会话状态")
}

#[test]
fn recovering_session_without_work_item_is_settled_to_idle() {
    let (_path, store) = store_at("idle");
    let host = host(&store, "host-recovering-idle");
    let session_id = session(&store, "session-recovering-idle");
    // 崩溃现场：会话被写成 recovering，但它名下一条在途工作项都没有。
    mark_recovering(&store, &session_id, 20);

    let settlement = store
        .settle_recovering_sessions(&host, 30, TTL_MS, 10)
        .expect("收敛 recovering");

    assert_eq!(settlement.settled_to_idle, 1, "没有在途工作项必须回 idle");
    assert_eq!(settlement.interrupted, 0, "回 idle 不是终态产出");
    assert!(
        settlement.events.is_empty(),
        "回 idle 没有要追加的终态事件：{}",
        settlement.events.len()
    );
    assert_eq!(
        store.get_session(&session_id).expect("读回会话").status,
        SessionRuntimeStatus::Idle,
        "recovering 必须落到 idle，不能停在原地"
    );
}

#[test]
fn recovering_session_past_ttl_settles_interrupted() {
    let (_path, store) = store_at("ttl");
    let host = host(&store, "host-recovering-ttl");
    let session_id = session(&store, "session-recovering-ttl");
    mark_recovering(&store, &session_id, 20);
    // 一条从未派发的排队工作项：没有租约，也没有执行者。
    insert_work_item(
        &store,
        "execution-recovering-ttl",
        &session_id,
        None,
        "queued",
        None,
        20,
    );

    // TTL 未到：什么都不做，给重新派发留出窗口。
    let early = store
        .settle_recovering_sessions(&host, 1_000, TTL_MS, 10)
        .expect("TTL 未到的收敛");
    assert_eq!(
        early,
        crate::ports::RecoveringSettlement::default(),
        "TTL 未到不得收口"
    );
    assert_eq!(session_status(&store, &session_id), "recovering");

    // TTL 到期：收口为中断。
    let settlement = store
        .settle_recovering_sessions(&host, 200_000, TTL_MS, 10)
        .expect("TTL 到期的收敛");
    assert_eq!(settlement.interrupted, 1, "过期未派发必须收口为中断");
    assert_eq!(settlement.settled_to_idle, 1, "收口后会话必须回 idle");
    assert_eq!(settlement.events.len(), 1, "收口必须追加一条会话事件");
    assert_eq!(session_status(&store, &session_id), "idle");

    // 终态行搬进历史表：在途表只留排队与运行中的行。
    let connection = store.connection().expect("connection");
    let in_flight: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM work_items WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get(0),
        )
        .expect("在途表可查");
    assert_eq!(in_flight, 0, "终态工作项不得留在在途表");
    let archived: (String, String) = connection
        .query_row(
            "SELECT status,terminal_kind FROM work_item_history WHERE work_id=?1",
            params!["execution-recovering-ttl"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("历史表应承接被收口的工作项");
    assert_eq!(archived.0, "failed", "中断落在失败状态上，由终态类别区分");
    assert_eq!(archived.1, "interrupted");

    // 事件面留证据：类型与载荷都要能回答「谁被判成中断、为什么」。
    let events = store.replay_events(&session_id, 0, 10).expect("回放事件面");
    let settled = events.last().expect("收口事件");
    assert_eq!(settled.event_type, "work_item_interrupted");
    assert!(
        settled.payload_json.contains("recovery_ttl_expired")
            && settled
                .payload_json
                .contains("\"executionId\":\"execution-recovering-ttl\""),
        "中断证据必须带原因与工作项标识：{}",
        settled.payload_json
    );
}

/// INV-7：会话状态机没有回不来的态。
///
/// 一轮收敛之后，只有「仍有有效租约在跑」的会话可以留在 `recovering`，
/// 其余两种形状（没有工作项、工作项过期未派发）必须全部落地。
#[test]
fn recovering_sessions_never_stay_without_an_exit() {
    let (_path, store) = store_at("inv7");
    let host = host(&store, "host-recovering-inv7");
    let idle_session = session(&store, "session-recovering-inv7-idle");
    mark_recovering(&store, &idle_session, 20);
    let expired_session = session(&store, "session-recovering-inv7-expired");
    mark_recovering(&store, &expired_session, 20);
    insert_work_item(
        &store,
        "execution-inv7-expired",
        &expired_session,
        None,
        "queued",
        None,
        20,
    );
    // 唯一可以留下的形状：工作项正在跑，租约还没过期。
    let running_session = session(&store, "session-recovering-inv7-running");
    mark_recovering(&store, &running_session, 20);
    insert_work_item(
        &store,
        "execution-inv7-running",
        &running_session,
        None,
        "running",
        Some(900_000),
        20,
    );

    store
        .settle_recovering_sessions(&host, 200_000, TTL_MS, 10)
        .expect("收敛 recovering");

    let stuck: Vec<String> = {
        let connection = store.connection().expect("connection");
        let mut statement = connection
            .prepare("SELECT session_id FROM sessions WHERE status='recovering'")
            .expect("准备查询");
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("查询 recovering 会话");
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .expect("读取结果")
    };
    assert_eq!(
        stuck,
        vec!["session-recovering-inv7-running".to_owned()],
        "只有租约有效的在跑会话可以留在 recovering"
    );
}

/// 真实崩溃序列：一轮 supervisor 之内必须从 `recovering` 落到 `idle`。
///
/// 照着 daemon 的固定顺序走一遍真实入口：接管失控会话 → 中断收敛 → `recovering` 收敛。
/// 顺序是这条链路的一部分：中断收敛只负责把工作项写成终态，会话状态要等到收敛才落地。
#[test]
fn crashed_turn_leaves_recovering_within_one_supervisor_tick() {
    let (_path, store) = store_at("crash");
    let old_host = store
        .claim_host(
            &RuntimeHostId::new("host-crash-old").expect("host"),
            1,
            None,
            10,
            100,
        )
        .expect("旧 Host lease");
    let session_id = session(&store, "session-crash");
    store
        .set_session_status(&session_id, &old_host, SessionRuntimeStatus::Running, 20)
        .expect("绑定旧 Host");
    // 用户提交过一轮：入站事实与在途工作项都在，随后进程消失。
    let interaction =
        crate::infrastructure::sqlite_runtime_store_interaction_fixtures::interaction_in_session(
            &session_id,
            "interaction-crash",
            "workspace-crash",
            "destination-crash",
            20,
        );
    store.append(&interaction).expect("入站事实");
    store
        .claim_processing_by_id(&interaction.interaction_id, 20, 100)
        .expect("领取")
        .expect("lease");

    let new_host = store
        .claim_host(
            &RuntimeHostId::new("host-crash-new").expect("host"),
            2,
            None,
            400,
            1_000_000,
        )
        .expect("新 Host lease");
    let recovered = store
        .recover_expired_sessions(&new_host, 400, 10)
        .expect("接管失控会话");
    assert_eq!(recovered.len(), 1, "失控会话必须被接管");
    assert_eq!(session_status(&store, &session_id), "recovering");

    // supervisor 同一轮的固定顺序：先中断收敛，再收 recovering。
    store
        .quarantine_interrupted_interactions(&new_host, 400, 10)
        .expect("中断收敛");
    let settlement = store
        .settle_recovering_sessions(&new_host, 400, TTL_MS, 10)
        .expect("recovering 收敛");

    assert_eq!(settlement.settled_to_idle, 1, "腾空的会话必须回 idle");
    assert_eq!(settlement.interrupted, 0, "工作项已由中断收敛处理过");
    assert_eq!(
        session_status(&store, &session_id),
        "idle",
        "崩溃后一轮之内必须离开 recovering"
    );
}
