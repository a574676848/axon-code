//! Gateway 桥接契约测试：默认工作区解析、游标往返，以及订阅滞后时的补齐与交棒。

use super::*;
use axon_domain::{DaemonSessionId, RuntimeHostId, SessionRuntimeStatus};
use axon_session::SqliteRuntimeStore;

/// 测试用页大小：故意远小于待补齐区间，让一次订阅必然跨页。
const TEST_REPLAY_PAGE_SIZE: u32 = 2;
/// 实时邮箱容量：需同时容纳被重放覆盖的重叠事件与订阅后的实时尾部。
const TEST_MAILBOX_CAPACITY: usize = 4;

#[test]
fn cursor_round_trip_uses_structured_json() {
    let cursor = ConsumerCursor::new("generation-a", 42);
    let encoded = serde_json::to_string(&cursor).expect("cursor 应可编码");
    assert_eq!(parse_cursor(&encoded).expect("cursor 应可解析"), cursor);
}

#[test]
fn malformed_cursor_is_rejected_before_subscription() {
    let error = parse_cursor("generation-a:42").expect_err("非 JSON cursor 应拒绝");
    assert!(
        matches!(error, GatewayKernelError::Ingress(message) if message.contains("resume_cursor"))
    );
}

#[test]
fn subscriber_lag_transparently_replays_missing_durable_events() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("gateway-reset-host").expect("Host ID");
    let host = store
        .claim_host(&host_id, 1, None, 10, 10_000)
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("gateway-reset-session").expect("Session ID");
    store
        .create_session(&session_id, ".", 10)
        .expect("创建 Session");
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 11)
        .expect("启动 Session");

    let interaction_id = InteractionId::new("gateway-reset-interaction").expect("Interaction ID");
    let stream_bridge = Arc::new(EventStreamBridge::new());
    let subscription = stream_bridge
        .subscribe_with_capacity(session_id.clone(), interaction_id.clone(), None, 1)
        .expect("订阅实时事件");
    for sequence in 1..=3 {
        let event = store
            .append_event(
                &session_id,
                &host,
                "wire_event",
                &format!(
                    "{{\"interactionId\":\"{}\",\"sequence\":{sequence}}}",
                    interaction_id.as_str()
                ),
                11 + sequence,
            )
            .expect("追加 durable 事件");
        stream_bridge.publish_for_interaction(&interaction_id, &event, &event.cursor());
    }

    let mut stream = DaemonGatewayEventStream {
        store,
        stream_bridge,
        session_id,
        interaction_id: Some(interaction_id),
        replay_page_size: TEST_REPLAY_PAGE_SIZE,
        host_generation: None,
        subscription,
        pending: VecDeque::new(),
        replay_cursor: None,
    };
    let sequences = (0..3)
        .map(|_| {
            let event = stream
                .recv_timeout(Duration::from_millis(50))
                .expect("订阅不应因落后失败")
                .expect("应收到事件");
            parse_cursor(event.cursor.as_deref().expect("事件应有 cursor"))
                .expect("cursor 应有效")
                .sequence
        })
        .collect::<Vec<_>>();

    assert_eq!(sequences, [1, 2, 3]);
}

#[test]
fn replay_hands_over_to_live_without_dropping_tail_events() {
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
    // 序号 1、2：无 interactionId 的事件，占满重放的第 1 页。
    for _ in 0..TEST_REPLAY_PAGE_SIZE {
        store
            .append_event(&session_id, &host, "wire_event", "{}", 12)
            .expect("追加 durable 事件");
    }
    // 序号 3：不匹配事件，用来占满重放的第 1 页。
    store
        .append_event(
            &session_id,
            &host,
            "wire_event",
            "{\"interactionId\":\"other-interaction\"}",
            12,
        )
        .expect("追加 durable 事件");
    // 序号 4：本回合事件，落在重放第 2 页。
    let replayed = store
        .append_event(
            &session_id,
            &host,
            "wire_event",
            &format!("{{\"interactionId\":\"{}\"}}", interaction_id.as_str()),
            12,
        )
        .expect("追加 durable 事件");
    let ceiling = live_ceiling_cursor(&store, &session_id, Some(host.generation.as_str()))
        .expect("读取 durable 尾");
    assert_eq!(ceiling.sequence, replayed.sequence);

    // 订阅建立后落 durable 的实时尾部：序号 5、6 只能靠实时流补齐。
    let stream_bridge = Arc::new(EventStreamBridge::new());
    let subscription = stream_bridge
        .subscribe_with_capacity(
            session_id.clone(),
            interaction_id.clone(),
            None,
            TEST_MAILBOX_CAPACITY,
        )
        .expect("订阅实时事件");
    let mut stream = DaemonGatewayEventStream {
        store,
        stream_bridge,
        session_id,
        interaction_id: Some(interaction_id),
        replay_page_size: TEST_REPLAY_PAGE_SIZE,
        host_generation: Some(host.generation.clone()),
        subscription,
        pending: VecDeque::new(),
        replay_cursor: None,
    };
    let (subscription, pending, replay_cursor) = subscribe_and_replay(
        &stream.store,
        &stream.stream_bridge,
        &stream.session_id,
        stream
            .interaction_id
            .as_ref()
            .expect("测试路径按 Interaction 订阅"),
        None,
        stream.replay_page_size,
        ceiling,
    )
    .expect("重放不应失败");
    stream.subscription = subscription;
    stream.pending = pending;
    stream.replay_cursor = replay_cursor;

    // 订阅后的实时尾部投影：序号 5、6 只能靠实时流补齐。
    let interaction_id = stream
        .interaction_id
        .clone()
        .expect("测试路径按 Interaction 订阅");
    let mut live_events = Vec::new();
    for _ in 0..TEST_REPLAY_PAGE_SIZE {
        live_events.push(
            stream
                .store
                .append_event(
                    &stream.session_id,
                    &host,
                    "wire_event",
                    &format!("{{\"interactionId\":\"{}\"}}", interaction_id.as_str()),
                    12,
                )
                .expect("追加订阅建立后的实时事件"),
        );
    }
    stream
        .stream_bridge
        .publish_for_interaction(&interaction_id, &replayed, &replayed.cursor());
    for event in &live_events {
        stream
            .stream_bridge
            .publish_for_interaction(&interaction_id, event, &event.cursor());
    }

    let expected_live_count = TEST_MAILBOX_CAPACITY - 1;
    let sequences = (0..expected_live_count)
        .map(|index| {
            let event = stream
                .recv_timeout(Duration::from_millis(50))
                .expect("订阅不应因重放截断失败")
                .expect("应收到事件");
            assert_eq!(
                event.replay,
                index == 0,
                "仅首条补齐事件属于回放，后续实时事件必须允许通知"
            );
            parse_cursor(event.cursor.as_deref().expect("事件应有 cursor"))
                .expect("cursor 应有效")
                .sequence
        })
        .collect::<Vec<_>>();

    // 重放补上序号 4，实时流补齐订阅后的尾部：水位若停在重放页而不是 durable 尾，
    // 邮箱里那条重叠事件会挤掉尾部事件，尾部序号也不会出现在结果里。
    let expected = std::iter::once(replayed.sequence)
        .chain(live_events.iter().map(|event| event.sequence))
        .collect::<Vec<_>>();
    assert_eq!(sequences, expected);
}
