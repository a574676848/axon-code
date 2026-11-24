//! WP-03 截断可见与读到尾（INV-5）的订阅契约。
//!
//! 三条断言对应三件事：载荷预算不再是「是否读到尾」的判定；真截断时回执带**真实**
//! 续读游标；按该游标续读既不跳帧也不重复。三条都在真实存储与真实订阅桥上跑，
//! 不使用任何桩件。

use axon_daemon::EventStreamBridge;
use axon_daemon::infrastructure::gateway_kernel::GatewayReplayReport;
use axon_daemon::infrastructure::gateway_replay::{
    ReplayReadLimits, subscribe_and_replay_with_limits,
};
use axon_daemon::infrastructure::gateway_websocket::subscription_receipt;
use axon_domain::{
    ConsumerCursor, DaemonSessionId, InteractionId, RuntimeHostId, SessionRuntimeStatus,
};
use axon_session::{DaemonHostRecord, EventLogStore, RuntimeStore, SqliteRuntimeStore};

/// 单页事件数：取小值让一次重放必然跨页。
const PAGE_SIZE: u32 = 2;
/// 会话里的事件总数：足够跨三页，且能区分「读到第 2 条」与「读到尾」。
const TOTAL_EVENTS: u64 = 6;

/// 建一条会话并写入 `TOTAL_EVENTS` 条带归属的帧，返回存储、会话、交互与 Host。
fn seed(
    directory: &std::path::Path,
    interaction_label: &str,
) -> (
    SqliteRuntimeStore,
    DaemonSessionId,
    InteractionId,
    DaemonHostRecord,
) {
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("gateway-ingest-host").expect("Host ID"),
            1,
            None,
            10,
            10_000,
        )
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("gateway-ingest-session").expect("Session ID");
    store
        .create_session(&session_id, ".", 10)
        .expect("创建 Session");
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 11)
        .expect("启动 Session");
    let interaction_id = InteractionId::new(interaction_label).expect("Interaction ID");
    for _ in 0..TOTAL_EVENTS {
        store
            .append_attributed_events(
                &session_id,
                &host,
                &interaction_id,
                &[(
                    "wire_event".to_owned(),
                    format!("{{\"interactionId\":\"{}\"}}", interaction_id.as_str()),
                )],
                12,
            )
            .expect("追加 durable 事件");
    }
    (store, session_id, interaction_id, host)
}

/// durable 尾游标：重放上界，也是「读到尾」的判据。
fn ceiling(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
) -> ConsumerCursor {
    let runtime = store.get_session(session_id).expect("会话应存在");
    ConsumerCursor::new(host.generation.clone(), runtime.last_event_sequence)
}

/// 从事件帧里取序号：游标就是 durable 事实的唯一读法。
fn frame_sequences(events: &std::collections::VecDeque<axon_daemon::GatewayEvent>) -> Vec<u64> {
    events
        .iter()
        .map(|event| {
            let raw = event.cursor.as_deref().expect("回放帧必须带游标");
            serde_json::from_str::<ConsumerCursor>(raw)
                .expect("游标应可解析")
                .sequence
        })
        .collect()
}

/// 载荷预算触顶只留告警，绝不中断读取：一次订阅必须读到 durable 尾。
#[test]
fn replay_reads_through_byte_budget_to_tail() {
    let directory = axon_test_support::test_temp_dir();
    let (store, session_id, interaction_id, host) = seed(directory.path(), "gateway-ingest-budget");
    let tail = ceiling(&store, &session_id, &host);
    let bridge = EventStreamBridge::new();
    // 预算取 1 字节：任何一帧都会超；页数给足，因此「读到尾」只能由页数判定。
    let limits = ReplayReadLimits {
        page_size: PAGE_SIZE,
        max_pages: 1_000,
        max_bytes: 1,
    };
    let (subscription, pending, replay_watermark, report) = subscribe_and_replay_with_limits(
        &store,
        &bridge,
        &session_id,
        &interaction_id,
        None,
        limits,
        tail.clone(),
    )
    .expect("重放不应失败");

    assert_eq!(
        frame_sequences(&pending),
        (1..=TOTAL_EVENTS).collect::<Vec<_>>(),
        "预算触顶不得中断读取：所有帧都必须回放出来"
    );
    assert!(
        !report.replay_truncated,
        "读到 durable 尾就不是截断：{report:?}"
    );
    // 未截断时续读游标就是 durable 尾，正常路径行为零变化。
    let resume = report
        .resume_cursor
        .as_deref()
        .expect("未截断也应有续读游标");
    let resume: ConsumerCursor = serde_json::from_str(resume).expect("续读游标应可解析");
    assert_eq!(resume, tail, "未截断时续读游标必须精确等于 durable 尾");
    assert_eq!(replay_watermark, Some(tail), "去重水位同样停在 durable 尾");
    drop(subscription);
}

/// 真截断时回执必须带真实已读序号，且不得拿 durable 尾冒充。
#[test]
fn replay_truncation_receipt_carries_resume_cursor() {
    let directory = axon_test_support::test_temp_dir();
    let (store, session_id, interaction_id, host) =
        seed(directory.path(), "gateway-ingest-truncated");
    let tail = ceiling(&store, &session_id, &host);
    let bridge = EventStreamBridge::new();
    // 只允许读一页：第一页两条之后就被页数硬上界截断。
    let limits = ReplayReadLimits {
        page_size: PAGE_SIZE,
        max_pages: 1,
        max_bytes: 64 * 1024 * 1024,
    };
    let (subscription, pending, replay_watermark, report) = subscribe_and_replay_with_limits(
        &store,
        &bridge,
        &session_id,
        &interaction_id,
        None,
        limits,
        tail.clone(),
    )
    .expect("重放不应失败");

    assert!(report.replay_truncated, "只读一页必须如实报截断");
    assert_eq!(
        frame_sequences(&pending),
        vec![1, 2],
        "截断页只含已读的两帧"
    );
    let resume_raw = report
        .resume_cursor
        .as_deref()
        .expect("截断必须给出续读游标");
    let resume: ConsumerCursor = serde_json::from_str(resume_raw).expect("续读游标应可解析");
    assert_eq!(
        resume.sequence, 2,
        "续读游标必须是本次真正重放到的那条帧序号，不得拿 durable 尾冒充"
    );
    assert_ne!(resume.sequence, tail.sequence, "确证不是 durable 尾");
    // 去重水位同样下调到真实已读位置：宁可重叠，也不让实时通道漏掉缺口那一段。
    assert_eq!(
        replay_watermark.as_ref().map(|cursor| cursor.sequence),
        Some(2)
    );

    let receipt = subscription_receipt("interaction:gateway-ingest", &report);
    assert_eq!(receipt["type"], "subscribed");
    assert_eq!(receipt["replayTruncated"], true);
    let receipt_cursor: ConsumerCursor = serde_json::from_str(
        receipt["resumeCursor"]
            .as_str()
            .expect("截断回执必须带 resumeCursor"),
    )
    .expect("回执游标应可解析");
    assert_eq!(receipt_cursor, resume, "回执里的续读游标必须与报告同源");

    // 未截断的确认帧不塞噪声字段。
    let clean = subscription_receipt(
        "interaction:gateway-ingest",
        &GatewayReplayReport::default(),
    );
    assert!(clean.get("replayTruncated").is_none(), "{clean}");
    assert!(clean.get("resumeCursor").is_none(), "{clean}");
    drop(subscription);
}

/// 按回执游标续读：不跳帧、不重复，两段拼起来正好是整条轮次。
#[test]
fn replay_resume_cursor_does_not_skip_frames() {
    let directory = axon_test_support::test_temp_dir();
    let (store, session_id, interaction_id, host) = seed(directory.path(), "gateway-ingest-resume");
    let tail = ceiling(&store, &session_id, &host);
    let bridge = EventStreamBridge::new();
    let (first_subscription, first_page, _, first_report) = subscribe_and_replay_with_limits(
        &store,
        &bridge,
        &session_id,
        &interaction_id,
        None,
        ReplayReadLimits {
            page_size: PAGE_SIZE,
            max_pages: 1,
            max_bytes: 64 * 1024 * 1024,
        },
        tail.clone(),
    )
    .expect("第一段重放不应失败");
    assert!(first_report.replay_truncated, "第一段必须是截断的");
    let resume: ConsumerCursor = serde_json::from_str(
        first_report
            .resume_cursor
            .as_deref()
            .expect("截断必须给出续读游标"),
    )
    .expect("续读游标应可解析");

    // 客户端按回执游标续订：起点就是上一次真正读到的位置。
    let (second_subscription, second_page, _, second_report) = subscribe_and_replay_with_limits(
        &store,
        &bridge,
        &session_id,
        &interaction_id,
        Some(resume.clone()),
        ReplayReadLimits::for_page_size(PAGE_SIZE),
        tail,
    )
    .expect("续读不应失败");

    let first = frame_sequences(&first_page);
    let second = frame_sequences(&second_page);
    assert_eq!(first, vec![1, 2], "第一段就是已读的两帧");
    assert_eq!(
        second,
        (3..=TOTAL_EVENTS).collect::<Vec<_>>(),
        "续读必须从已读位置的下一条开始：既不跳帧，也不重复第一段"
    );
    assert!(
        !second_report.replay_truncated,
        "续读一次就要读到尾，否则缺口会一直留着"
    );
    let mut all = first;
    all.extend(second);
    assert_eq!(
        all,
        (1..=TOTAL_EVENTS).collect::<Vec<_>>(),
        "两段拼起来必须是无洞无重复的整条轮次"
    );
    drop(first_subscription);
    drop(second_subscription);
}

/// 归属只在归属列上、载荷里没有 `interactionId` 的帧，订阅回放也必须读得到。
///
/// 子代理的逐轮正文就是这种形状：sink 写归属列，载荷里不重复写一遍。旧实现按载荷
/// 过滤，于是子代理详情一条帧都收不到（现场表现为空白）。
#[test]
fn replay_reads_frames_attributed_only_by_column() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("gateway-column-host").expect("Host ID"),
            1,
            None,
            10,
            10_000,
        )
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("gateway-column-session").expect("Session ID");
    store
        .create_session(&session_id, ".", 10)
        .expect("创建 Session");
    store
        .set_session_status(&session_id, &host, SessionRuntimeStatus::Running, 11)
        .expect("启动 Session");
    let interaction_id = InteractionId::new("gateway-column-interaction").expect("Interaction ID");
    for index in 0..3 {
        store
            .append_attributed_events(
                &session_id,
                &host,
                &interaction_id,
                &[(
                    "wire_event".to_owned(),
                    format!("{{\"text\":\"帧 {index}\"}}"),
                )],
                12,
            )
            .expect("追加 durable 事件");
    }
    let tail = ceiling(&store, &session_id, &host);
    let bridge = EventStreamBridge::new();
    let limits = ReplayReadLimits {
        page_size: PAGE_SIZE,
        max_pages: 1_000,
        max_bytes: 64 * 1024 * 1024,
    };
    let (subscription, pending, _, report) = subscribe_and_replay_with_limits(
        &store,
        &bridge,
        &session_id,
        &interaction_id,
        None,
        limits,
        tail,
    )
    .expect("重放不应失败");

    assert_eq!(
        frame_sequences(&pending),
        (1..=3).collect::<Vec<_>>(),
        "只有归属列的帧也必须回放出来"
    );
    assert!(!report.replay_truncated, "三帧不足一页预算，不该截断");
    drop(subscription);
}
