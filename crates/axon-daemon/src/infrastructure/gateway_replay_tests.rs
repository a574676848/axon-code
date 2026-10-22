//! Gateway durable replay 契约测试：重放必须补齐到 durable 尾。
//!
//! 另有一组回归：订阅早已终态的 Interaction 时，回放必须以一条终态帧收口。
//! 没有它，界面会停在「生成中」（这一轮早已结束，durable 日志里却没有收口事实）。

use super::*;
use crate::application::EventStreamBridge;
use crate::infrastructure::gateway_bridge::parse_cursor;
use axon_domain::{
    AudienceContext, AudienceKind, ConsumerCursor, DaemonSessionId, DisclosurePolicy,
    InteractionId, InteractionRecord, InteractionSource, InteractionStatus, ReplyRoute,
    RuntimeHostId, ScopeRef, SessionRuntimeStatus,
};
use axon_session::{DaemonHostRecord, InteractionStore, RuntimeStore, SqliteRuntimeStore};

/// 测试用页大小：故意取小值，让一次订阅必然跨多页。
const TEST_REPLAY_PAGE_SIZE: u32 = 2;
/// 待补齐区间跨越的页数。
const SCANNED_PAGE_COUNT: u32 = 3;

/// 用真实存储固定一次跨页重放：单页上限是 window，不是总量。
#[test]
fn replay_reads_to_durable_ceiling_across_pages() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("gateway-page-host").expect("Host ID");
    let host = store
        .claim_host(&host_id, 1, None, 10, 10_000)
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("gateway-page-session").expect("Session ID");
    store
        .create_session(&session_id, ".", 10)
        .expect("创建 Session");
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 11)
        .expect("启动 Session");

    let interaction_id = InteractionId::new("gateway-page-interaction").expect("Interaction ID");
    for _ in 0..2 {
        store
            .append_event(&session_id, &host, "wire_event", "{}", 12)
            .expect("追加 durable 事件");
    }
    // 前两条不匹配的事件占满第 1 页（页大小 2），本回合的事件从第 2 页起。
    for payload in [
        "{\"interactionId\":\"other-interaction\"}".to_owned(),
        "{\"interactionId\":\"other-interaction\"}".to_owned(),
        format!("{{\"interactionId\":\"{}\"}}", interaction_id.as_str()),
        format!("{{\"interactionId\":\"{}\"}}", interaction_id.as_str()),
    ] {
        store
            .append_event(&session_id, &host, "wire_event", &payload, 12)
            .expect("追加 durable 事件");
    }
    let ceiling = live_ceiling_cursor(&store, &session_id, Some(host.generation.as_str()))
        .expect("读取 durable 尾");
    assert_eq!(ceiling.sequence, 6);

    let stream_bridge = EventStreamBridge::new();
    let (subscription, pending, replay_cursor) = subscribe_and_replay(
        &store,
        &stream_bridge,
        &session_id,
        &interaction_id,
        None,
        TEST_REPLAY_PAGE_SIZE,
        ceiling.clone(),
    )
    .expect("重放不应失败");

    // 重放水位必须停在 durable 尾，而不是最后一页匹配事件的序号。
    assert_eq!(replay_cursor, Some(ceiling));
    let sequences = pending
        .iter()
        .map(|event| {
            parse_cursor(event.cursor.as_deref().expect("事件应有 cursor"))
                .expect("cursor 应有效")
                .sequence
        })
        .collect::<Vec<_>>();
    assert_eq!(sequences, [5, 6]);
    assert!(
        pending.iter().all(|event| event.replay),
        "跨页回放必须保留历史标记"
    );
    drop(subscription);
}

/// 单页上限远小于待补齐区间时，水位仍须越过整段区间。
#[test]
fn replay_watermark_covers_every_scanned_page() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("gateway-scan-host").expect("Host ID");
    let host = store
        .claim_host(&host_id, 1, None, 10, 10_000)
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("gateway-scan-session").expect("Session ID");
    store
        .create_session(&session_id, ".", 10)
        .expect("创建 Session");
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 11)
        .expect("启动 Session");

    let interaction_id = InteractionId::new("gateway-scan-interaction").expect("Interaction ID");
    let total = TEST_REPLAY_PAGE_SIZE * SCANNED_PAGE_COUNT;
    for _ in 0..total {
        store
            .append_event(
                &session_id,
                &host,
                "wire_event",
                "{\"interactionId\":\"other-interaction\"}",
                12,
            )
            .expect("追加 durable 事件");
    }
    let ceiling = live_ceiling_cursor(&store, &session_id, Some(host.generation.as_str()))
        .expect("读取 durable 尾");
    assert_eq!(ceiling.sequence, u64::from(total));

    let stream_bridge = EventStreamBridge::new();
    let (subscription, pending, replay_cursor) = subscribe_and_replay(
        &store,
        &stream_bridge,
        &session_id,
        &interaction_id,
        Some(ConsumerCursor::new(host.generation.as_str(), 0)),
        TEST_REPLAY_PAGE_SIZE,
        ceiling.clone(),
    )
    .expect("重放不应失败");

    assert!(pending.is_empty(), "无匹配事件时不应产出事件");
    assert_eq!(replay_cursor, Some(ceiling));
    drop(subscription);
}

/// 回归：长会话里最新的 Interaction 必须照样读到（页数上限曾让事件过万的会话
/// 只回 `subscribed`、桌面端正文整屏空白并一直停在生成中）。
#[test]
fn replay_reaches_the_newest_interaction_of_a_long_session() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("gateway-long-host").expect("Host ID");
    let host = store
        .claim_host(&host_id, 1, None, 10, 10_000)
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("gateway-long-session").expect("Session ID");
    store
        .create_session(&session_id, ".", 10)
        .expect("创建 Session");
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 11)
        .expect("启动 Session");

    // 前缀事件数远超旧口径的续传总量（页数上限 × 页大小 = 100 × 2）。
    let prefix = TEST_REPLAY_PAGE_SIZE * 130;
    let interaction_id = InteractionId::new("gateway-newest-interaction").expect("Interaction ID");
    for _ in 0..prefix {
        store
            .append_event(
                &session_id,
                &host,
                "wire_event",
                "{\"interactionId\":\"other-interaction\"}",
                12,
            )
            .expect("追加前缀事件");
    }
    for _ in 0..2 {
        store
            .append_event(
                &session_id,
                &host,
                "wire_event",
                &format!("{{\"interactionId\":\"{}\"}}", interaction_id.as_str()),
                12,
            )
            .expect("追加目标 Interaction 事件");
    }
    let ceiling = live_ceiling_cursor(&store, &session_id, Some(host.generation.as_str()))
        .expect("读取 durable 尾");
    assert_eq!(ceiling.sequence, u64::from(prefix) + 2);

    let stream_bridge = EventStreamBridge::new();
    let (subscription, pending, _) = subscribe_and_replay(
        &store,
        &stream_bridge,
        &session_id,
        &interaction_id,
        None,
        TEST_REPLAY_PAGE_SIZE,
        ceiling,
    )
    .expect("重放不应失败");

    let sequences = pending
        .iter()
        .map(|event| {
            parse_cursor(event.cursor.as_deref().expect("事件应有 cursor"))
                .expect("cursor 应有效")
                .sequence
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sequences,
        [u64::from(prefix) + 1, u64::from(prefix) + 2],
        "最新 Interaction 的事件不得被续传总量截断"
    );
    drop(subscription);
}

/// 建一条车道会话与一条 Interaction，并把它的状态推到 `status`。
///
/// 状态跃迁走生产同一套路径（append → claim → mark）：测试造的事实必须与真实
/// 存储一致，否则补帧逻辑的边界就成了自说自话。
fn seed_interaction(
    directory: &std::path::Path,
    status: InteractionStatus,
    interaction_id: &str,
) -> (
    SqliteRuntimeStore,
    InteractionId,
    DaemonSessionId,
    DaemonHostRecord,
) {
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("gateway-terminal-host").expect("Host ID");
    let host = store
        .claim_host(&host_id, 1, None, 10, 10_000)
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("gateway-terminal-session").expect("Session ID");
    store
        .create_session(&session_id, ".", 10)
        .expect("创建 Session");
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 11)
        .expect("启动 Session");

    let interaction_id = InteractionId::new(interaction_id).expect("Interaction ID");
    let record = InteractionRecord::new(
        interaction_id.clone(),
        InteractionSource::new("desktop", format!("message-{}", interaction_id.as_str()))
            .expect("有效来源"),
        ScopeRef::workspace("workspace-terminal").expect("有效作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("desktop", "workbench-terminal").expect("有效路由"),
        "订阅一条早已结束的交互",
        12,
    )
    .expect("Interaction 有效");
    store.append(&record).expect("写入 Interaction");

    if status != InteractionStatus::Received {
        let lease = store
            .claim_processing_by_id(&interaction_id, 13, 10_000)
            .expect("领取不应失败")
            .expect("应有 processing lease");
        let now_ms = 14;
        let transition = match status {
            InteractionStatus::Processing => Ok(()),
            InteractionStatus::Understood => {
                store.mark_understood(&interaction_id, &lease.lease_token, now_ms)
            }
            InteractionStatus::Rejected => {
                store.mark_rejected(&interaction_id, &lease.lease_token, now_ms)
            }
            InteractionStatus::Interrupted => {
                store.mark_interrupted(&interaction_id, &lease.lease_token, now_ms)
            }
            InteractionStatus::Received => unreachable!("已在上面排除"),
        };
        transition.expect("状态跃迁应成功");
    }
    (store, interaction_id, session_id, host)
}

/// 订阅一条早已终态、且没有任何事件的 Interaction：回放必须以终态帧收口。
///
/// 这是「会话早就结束、界面却一直停在生成中」的成因：durable 日志里没有收口事实，
/// 实时通道也不会为早已结束的回合再发一条。
#[test]
fn replay_appends_terminal_status_for_finished_interaction_without_events() {
    let directory = axon_test_support::test_temp_dir();
    let (store, interaction_id, session_id, host) = seed_interaction(
        directory.path(),
        InteractionStatus::Interrupted,
        "gateway-terminal-interaction",
    );
    let ceiling = live_ceiling_cursor(&store, &session_id, Some(host.generation.as_str()))
        .expect("读取 durable 尾");
    let stream_bridge = EventStreamBridge::new();
    let (subscription, pending, _) = subscribe_and_replay(
        &store,
        &stream_bridge,
        &session_id,
        &interaction_id,
        None,
        TEST_REPLAY_PAGE_SIZE,
        ceiling,
    )
    .expect("重放不应失败");

    let events = pending.into_iter().collect::<Vec<_>>();
    assert_eq!(events.len(), 1, "没有 durable 事件时只应回放一条终态帧");
    let terminal = &events[0];
    assert_eq!(terminal.event_type, INTERACTION_STATUS_EVENT);
    assert_eq!(terminal.payload_json, "\"interrupted\"");
    assert!(terminal.replay, "补帧由订阅时的存储状态派生，属于回放补齐");
    assert!(
        terminal.cursor.is_none(),
        "补帧不是 durable 事件，不得给出游标"
    );
    drop(subscription);
}

/// 终态帧必须排在回放末尾：客户端按「最后一条未收口的轮次」归约，
/// 先收口会让随后回放的正文落到上一轮去。
#[test]
fn replay_keeps_terminal_status_after_replayed_history() {
    let directory = axon_test_support::test_temp_dir();
    let (store, interaction_id, session_id, host) = seed_interaction(
        directory.path(),
        InteractionStatus::Understood,
        "gateway-understood-interaction",
    );
    for sequence in 0..2 {
        store
            .append_event(
                &session_id,
                &host,
                "wire_event",
                &format!(
                    "{{\"interactionId\":\"{}\",\"sequence\":{sequence}}}",
                    interaction_id.as_str()
                ),
                15,
            )
            .expect("追加 durable 事件");
    }
    let ceiling = live_ceiling_cursor(&store, &session_id, Some(host.generation.as_str()))
        .expect("读取 durable 尾");
    let stream_bridge = EventStreamBridge::new();
    let (subscription, pending, _) = subscribe_and_replay(
        &store,
        &stream_bridge,
        &session_id,
        &interaction_id,
        None,
        TEST_REPLAY_PAGE_SIZE,
        ceiling,
    )
    .expect("重放不应失败");

    let events = pending.into_iter().collect::<Vec<_>>();
    assert_eq!(events.len(), 3, "两条正文事件之后才是终态帧");
    assert!(
        events[..2]
            .iter()
            .all(|event| event.event_type == "wire_event"),
        "正文事件必须原样回放在最前面"
    );
    assert_eq!(events[2].event_type, INTERACTION_STATUS_EVENT);
    assert_eq!(events[2].payload_json, "\"understood\"");
    drop(subscription);
}

/// 还在跑的 Interaction 不补帧：那一轮由实时通道收口，提前补一条会把它当成已结束。
#[test]
fn replay_leaves_running_interaction_open() {
    for status in [InteractionStatus::Received, InteractionStatus::Processing] {
        let directory = axon_test_support::test_temp_dir();
        let (store, interaction_id, session_id, host) =
            seed_interaction(directory.path(), status, "gateway-running-interaction");
        let ceiling = live_ceiling_cursor(&store, &session_id, Some(host.generation.as_str()))
            .expect("读取 durable 尾");
        let stream_bridge = EventStreamBridge::new();
        let (subscription, pending, _) = subscribe_and_replay(
            &store,
            &stream_bridge,
            &session_id,
            &interaction_id,
            None,
            TEST_REPLAY_PAGE_SIZE,
            ceiling,
        )
        .expect("重放不应失败");

        assert!(pending.is_empty(), "{status:?} 不是终态，不得补终态帧");
        drop(subscription);
    }
}
