//! INV-4 归属不变式在事件日志端口上的契约。
//!
//! 逐轮正文（`wire_event`）没有归属就落盘，等于把它写进了一个任何轮次视图都读不到的
//! 角落：子代理详情因此整页空白。契约是「无归属入口只接受会话级事件」，而「会话级」由
//! 会话类型与事件类型共同判定：
//!
//! - 子代理会话：逐字稿只经 Interaction 视图读取，逐轮正文必须带归属。
//! - 主会话（以及定时、目标会话）：回合之外的迟到作答没有可归属的轮次，按会话级放行，
//!   但留结构化日志与计数。这条放行是产品行为，收紧它会制造「永远待答的提问」。

use axon_domain::{DaemonSessionId, InteractionId, RuntimeHostId, SessionRuntimeStatus};
use axon_session::{
    DaemonHostRecord, EventLogStore, NewSessionRequest, RuntimeStore, SessionKind,
    SqliteRuntimeStore,
};

/// 建一条已绑定 Host 的会话：后续写入全部走真实存储路径。
fn seeded_session(
    kind: SessionKind,
) -> (
    axon_test_support::TestTempDir,
    SqliteRuntimeStore,
    DaemonHostRecord,
    DaemonSessionId,
) {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("attribution-host").expect("Host ID"),
            1,
            None,
            10,
            10_000,
        )
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("01J9Z0000000000000000000SE").expect("Session ID");
    let root_session_id = DaemonSessionId::new("01J9Z0000000000000000000RT").expect("根会话 ID");
    if kind == SessionKind::Subagent {
        store
            .create_session(&root_session_id, ".", 10)
            .expect("登记根会话");
        store
            .create_session_with(&NewSessionRequest {
                kind,
                root_session_id: Some(root_session_id),
                agent_path: Some("root/delegate".to_owned()),
                ..NewSessionRequest::primary(session_id.clone(), ".", 10)
            })
            .expect("登记子代理会话");
    } else {
        store
            .create_session(&session_id, ".", 10)
            .expect("创建 Session");
    }
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 11)
        .expect("启动 Session");
    (directory, store, host, session_id)
}

/// 子代理会话 + 无归属 `wire_event` → 拒绝，且不得留下任何帧。
#[test]
fn wire_event_without_attribution_is_rejected() {
    let (_directory, store, host, session_id) = seeded_session(SessionKind::Subagent);
    let before = <SqliteRuntimeStore as EventLogStore>::session_level_wire_writes();
    let error = store
        .append_events(
            &session_id,
            &host,
            &[(
                "wire_event".to_owned(),
                "{\"text\":\"没有归属\"}".to_owned(),
            )],
            20,
        )
        .expect_err("子代理会话上无归属的 wire_event 必须被拒绝");
    assert!(
        error.to_string().contains("wire_event"),
        "错误信息必须点明是哪种事件缺归属：{error}"
    );
    let error = store
        .append_event(&session_id, &host, "wire_event", "{}", 21)
        .expect_err("单条入口同样必须拒绝");
    assert!(error.to_string().contains("归属"), "{error}");
    // 拒绝必须发生在落盘之前：序号没有推进，日志里一条都不该有。
    assert!(
        store
            .replay_events(&session_id, 0, 10)
            .expect("回放")
            .is_empty(),
        "被拒绝的事件不得留下任何帧"
    );
    assert_eq!(
        <SqliteRuntimeStore as EventLogStore>::session_level_wire_writes(),
        before,
        "拒绝不计数：放行才计数"
    );
}

/// 子代理会话 + 带归属的 `wire_event` → 正常落盘，且按 Interaction 可读。
#[test]
fn attributed_wire_event_is_written_for_subagent_sessions() {
    let (_directory, store, host, session_id) = seeded_session(SessionKind::Subagent);
    let interaction_id = InteractionId::new("attribution-interaction").expect("Interaction ID");
    store
        .append_attributed_events(
            &session_id,
            &host,
            &interaction_id,
            &[("wire_event".to_owned(), "{}".to_owned())],
            22,
        )
        .expect("带归属的 wire_event 必须可写");
    let frames = store
        .replay_interaction_events(&session_id, &interaction_id, 0, 10)
        .expect("按归属回放");
    assert_eq!(frames.len(), 1, "带归属的帧必须能被轮次视图读到");
}

/// 主会话 + 无归属 `wire_event` → 放行（回合之外的迟到作答），并计数。
///
/// 这条断言锁的是白名单本身：收紧它会直接打死迟到作答的 durable 兜底，界面上会留下一条
/// 永远待答的提问。
#[test]
fn session_level_wire_event_is_allowed_on_primary_sessions() {
    let (_directory, store, host, session_id) = seeded_session(SessionKind::Primary);
    let before = <SqliteRuntimeStore as EventLogStore>::session_level_wire_writes();
    let written = store
        .append_events(
            &session_id,
            &host,
            &[(
                "wire_event".to_owned(),
                "{\"text\":\"迟到作答\"}".to_owned(),
            )],
            20,
        )
        .expect("主会话的会话级 wire 写入必须放行");
    assert_eq!(written.len(), 1);
    assert_eq!(
        <SqliteRuntimeStore as EventLogStore>::session_level_wire_writes(),
        before + 1,
        "放行必须计数：这条路径被放行过要可观测"
    );
    assert_eq!(
        store.replay_events(&session_id, 0, 10).expect("回放").len(),
        1,
        "放行的帧必须真的落盘"
    );
}

/// 会话级事件走无归属入口仍然可写：白名单不能被归属要求误伤。
#[test]
fn session_level_events_still_write_without_attribution() {
    let (_directory, store, host, session_id) = seeded_session(SessionKind::Subagent);
    for (offset, event_type) in [
        "job_started",
        "job_completed",
        "session_created",
        "session_recovered",
        "turn_metrics",
    ]
    .into_iter()
    .enumerate()
    {
        store
            .append_events(
                &session_id,
                &host,
                &[(event_type.to_owned(), "{}".to_owned())],
                30 + u64::try_from(offset).expect("偏移"),
            )
            .unwrap_or_else(|error| panic!("会话级事件 {event_type} 必须可写：{error}"));
    }
    assert_eq!(
        store.replay_events(&session_id, 0, 10).expect("回放").len(),
        5,
        "白名单里的会话级事件必须全部落盘"
    );
}
