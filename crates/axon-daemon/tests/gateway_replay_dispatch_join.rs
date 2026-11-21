//! WP-06 读侧：子代理轮次的帧经「同一条派发 join」可解析（INV-4 的读侧镜像）。
//!
//! 老记录的子代理帧没有归属列，按 Interaction 指纹一条都命中不到；子代理会话里整段帧
//! 就是那条派发轮次的正文，因此第二级按「子会话的派发唯一」把整段会话认成这条轮次。
//! 归属不唯一时必须如实报「没有归属」，不得猜测；载荷子串扫描已删除，不允许它复活。

use axon_domain::{
    AGENT_DISPATCH_SOURCE_CHANNEL, AudienceContext, AudienceKind, DaemonSessionId,
    DisclosurePolicy, InteractionId, InteractionRecord, InteractionSource, ReplyRoute,
    RuntimeHostId, ScopeRef, SessionRuntimeStatus,
};
use axon_session::{
    DaemonHostRecord, EventLogStore, InteractionStore, RuntimeStore, SqliteRuntimeStore,
};

/// 派发轮次的来源键前缀：与生产写入方共用同一份定义。
use axon_domain::AGENT_DISPATCH_SOURCE_PREFIX;

/// 一条子代理会话的现场：父会话、子会话、派发轮次与 Host。
struct Fixture {
    store: SqliteRuntimeStore,
    host: DaemonHostRecord,
    child_session: DaemonSessionId,
    dispatch: InteractionId,
}

/// 建父会话与子会话，并登记一条派发轮次。
fn fixture(directory: &std::path::Path, invocation: &str) -> Fixture {
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("gateway-dispatch-host").expect("Host ID"),
            1,
            None,
            10,
            10_000,
        )
        .expect("注册 Host");
    let parent = DaemonSessionId::new("01J9Z0000000000000000000PA").expect("父会话 ID");
    store
        .create_session_with(&axon_session::NewSessionRequest {
            workspace_id: Some("workspace-dispatch".to_owned()),
            ..axon_session::NewSessionRequest::primary(parent.clone(), ".", 10)
        })
        .expect("登记父会话");
    let child_session = DaemonSessionId::new("01J9Z0000000000000000000CH").expect("子会话 ID");
    store
        .create_session_with(&axon_session::NewSessionRequest {
            kind: axon_session::SessionKind::Subagent,
            workspace_id: Some("workspace-dispatch".to_owned()),
            root_session_id: Some(parent.clone()),
            agent_path: Some("root/delegate".to_owned()),
            ..axon_session::NewSessionRequest::primary(child_session.clone(), ".", 10)
        })
        .expect("登记子会话");
    store
        .set_session_status(&child_session, &host, SessionRuntimeStatus::Running, 11)
        .expect("启动子会话");
    Fixture {
        store,
        host,
        child_session,
        dispatch: InteractionId::new(format!("dispatch-{invocation}")).expect("派发轮次 ID"),
    }
}

/// 登记一条派发轮次：`inbox.session_id` 就是子会话。
fn append_dispatch(fixture: &Fixture, interaction_id: &InteractionId, now_ms: u64) {
    let record = InteractionRecord::new(
        fixture.child_session.clone(),
        interaction_id.clone(),
        InteractionSource::new(
            AGENT_DISPATCH_SOURCE_CHANNEL,
            format!(
                "{}{}:{}",
                AGENT_DISPATCH_SOURCE_PREFIX,
                "01J9Z0000000000000000000PA",
                interaction_id.as_str()
            ),
        )
        .expect("派发来源"),
        ScopeRef::workspace("workspace-dispatch").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("desktop", "workbench-dispatch").expect("回复路由"),
        "子代理的任务提示",
        now_ms,
    )
    .expect("派发轮次");
    fixture.store.append(&record).expect("写入派发轮次");
}

/// 老记录：没有归属列，也没有 `interactionId` 字段。
fn append_legacy_frames(fixture: &Fixture, count: u64) {
    for offset in 0..count {
        fixture
            .store
            .append_events(
                &fixture.child_session,
                &fixture.host,
                &[(
                    "tool_call".to_owned(),
                    format!("{{\"text\":\"legacy-{offset}\"}}"),
                )],
                20 + offset,
            )
            .expect("追加无归属的老帧");
    }
}

/// 子代理会话里只有一条派发时，整段会话帧就是这条轮次的正文。
#[test]
fn subagent_frames_resolve_through_dispatch_join() {
    let directory = axon_test_support::test_temp_dir();
    let fixture = fixture(directory.path(), "invocation-1");
    append_dispatch(&fixture, &fixture.dispatch, 12);
    append_legacy_frames(&fixture, 3);

    let page = axon_daemon::infrastructure::gateway_replay::interaction_history_page(
        &fixture.store,
        &fixture.dispatch,
        0,
        50,
        false,
    )
    .expect("历史页");
    assert_eq!(
        page.events.len(),
        3,
        "派发 join 必须把整段会话帧认成这条轮次的过程"
    );
    assert!(
        page.events
            .iter()
            .all(|event| event.payload_json.contains("legacy-")),
        "只应返回这条派发自己的帧"
    );
    assert!(
        page.unattributed_reason.is_none(),
        "能唯一确定归属时不得报「没有归属」"
    );
    assert!(!page.has_more, "整段会话已读完");

    // 尾窗路径同样走派发 join：桌面打开子代理详情读的就是这一条。
    let tail = axon_daemon::infrastructure::gateway_replay::interaction_history_page(
        &fixture.store,
        &fixture.dispatch,
        0,
        50,
        true,
    )
    .expect("尾窗");
    assert_eq!(tail.events.len(), 3, "尾窗也要能读到派发 join 的帧");
}

/// 同一子会话有多条派发时归属不唯一：如实报「没有归属」，不得猜测。
#[test]
fn ambiguous_dispatch_join_reports_missing_attribution() {
    let directory = axon_test_support::test_temp_dir();
    let fixture = fixture(directory.path(), "invocation-1");
    append_dispatch(&fixture, &fixture.dispatch, 12);
    append_legacy_frames(&fixture, 2);
    // 同一个子会话被再次派发：老帧属于哪一轮无法判定。
    let second = InteractionId::new("dispatch-invocation-2").expect("第二条派发轮次");
    append_dispatch(&fixture, &second, 30);

    let page = axon_daemon::infrastructure::gateway_replay::interaction_history_page(
        &fixture.store,
        &fixture.dispatch,
        0,
        50,
        false,
    )
    .expect("历史页");
    assert!(
        page.events.is_empty(),
        "归属不唯一时不得把整段会话帧挂到某一条轮次上：{:?}",
        page.events
    );
    let reason = page
        .unattributed_reason
        .as_deref()
        .expect("必须如实给出「没有归属」的原因");
    assert!(
        reason.contains("没有归属"),
        "原因必须点明归属缺失：{reason}"
    );
}
