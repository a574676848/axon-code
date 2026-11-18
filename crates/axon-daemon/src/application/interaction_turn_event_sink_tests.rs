//! `interaction_turn_event_sink` 的回归用例：实时帧与边界快照的分流。

use super::*;
use serde_json::json;

use std::sync::atomic::{AtomicU64, Ordering};

use axon_domain::{
    AudienceContext, AudienceKind, ConsumerCursor, DaemonSessionId, DisclosurePolicy,
    InteractionRecord, InteractionSource, ReplyRoute, RuntimeHostId, ScopeRef,
};
use axon_session::{EventLogStore, InteractionStore, SessionEventRecord, SqliteRuntimeStore};
use axon_session_runtime::EventPublisher;

use super::super::child_session_event_sink::ChildSessionSinkFactory;
use crate::application::event_stream_bridge::EventStreamBridge;

fn interaction_id() -> InteractionId {
    InteractionId::new("interaction-projection-test").expect("Interaction ID 应合法")
}

fn wire_payload(kind_type: &str, payload: Value) -> String {
    json!({
        "schemaVersion": "v1",
        "eventId": "event-1",
        "correlationId": "assistant",
        "session": {"sessionId": "session-1"},
        "sequence": 0,
        "recordedAtMs": 1,
        "kind": {"type": kind_type, "payload": payload},
    })
    .to_string()
}

#[test]
fn stream_event_annotation_adds_only_interaction_id() {
    let payload = annotate_live_event(
        r#"{"kind":{"type":"assistantMessage","payload":{}}}"#,
        &interaction_id(),
    )
    .expect("流事件应可标注");
    let payload: Value = serde_json::from_str(&payload).expect("payload 应合法");

    assert_eq!(payload["interactionId"], "interaction-projection-test");
    assert!(payload.get("transportContext").is_none());
}

#[test]
fn stream_deltas_stay_off_the_durable_path_and_usage_keeps_a_replay_copy() {
    let text = wire_payload(
        "reasoningDelta",
        json!({"partId": "reasoning-part:1", "reasoningId": "r1", "delta": "想"}),
    );
    assert!(matches!(
        classify_frame(&text),
        FrameClass::LivePart {
            kind: LivePartKind::Reasoning,
            ..
        }
    ));

    // 用量必须能回放：切回会话时这一轮读数只可能来自持久副本。
    let usage = wire_payload("usageUpdate", json!({"inputTokens": 1}));
    assert!(matches!(classify_frame(&usage), FrameClass::Snapshot));

    // 供应商状态是瞬态读数，不进持久层。
    let provider = wire_payload("providerStatus", json!({"activity": "thinking"}));
    assert!(matches!(
        classify_frame(&provider),
        FrameClass::LiveOnly { remember: true }
    ));

    let tool_call = wire_payload("toolCall", json!({"partId": "tool-1"}));
    assert!(matches!(classify_frame(&tool_call), FrameClass::Durable));

    let canonical = wire_payload(
        "assistantMessage",
        json!({"partId": "assistant-1", "content": "done"}),
    );
    assert!(matches!(classify_frame(&canonical), FrameClass::Durable));
}

#[test]
fn snapshot_reuses_the_same_part_envelope() {
    let first = wire_payload(
        "reasoningDelta",
        json!({"partId": "r-part", "reasoningId": "r1", "delta": "先想"}),
    );
    let envelope: Value = serde_json::from_str(&first).expect("信封");
    assert_eq!(
        live_part_text(&envelope, LivePartKind::Reasoning).as_deref(),
        Some("先想")
    );

    let assistant = wire_payload(
        "assistantMessage",
        json!({"partId": "a-part", "messageId": "m1", "content": "答", "delta": true}),
    );
    let envelope: Value = serde_json::from_str(&assistant).expect("信封");
    assert_eq!(
        live_part_text(&envelope, LivePartKind::Assistant).as_deref(),
        Some("答")
    );
}

fn runtime_event(event_type: &str, payload_json: String) -> RuntimeEvent {
    RuntimeEvent {
        event_type: event_type.to_owned(),
        payload_json,
    }
}

#[test]
fn turn_terminal_is_only_the_completion_event() {
    assert!(is_turn_terminal(&runtime_event(
        "turn_completed",
        "{}".to_owned()
    )));

    // 一条 provider 流收口也是 `status=completed` 的 turnLifecycle 帧。把它当轮收口，
    // turn_metrics 就会在第一段定稿（每轮只写一条），同轮后续所有流的用量全部丢失。
    for status in ["completed", "failed", "cancelled", "aborted"] {
        let streaming = wire_payload(
            "turnLifecycle",
            json!({ "status": status, "kind": "StreamingCompleted" }),
        );
        assert!(
            !is_turn_terminal(&runtime_event("wire_event", streaming)),
            "{status} 的单条流收口不是轮收口"
        );
    }

    // 整轮生命周期同样不承担收口判定：真正收口由 `turn_completed` 表达，
    // 两处都认会在同一轮写出两次指标。
    let run_lifecycle = wire_payload(
        "turnLifecycle",
        json!({ "status": "completed", "kind": "RunLifecycle" }),
    );
    assert!(!is_turn_terminal(&runtime_event(
        "wire_event",
        run_lifecycle
    )));

    // 运行中的生命周期不是收口：误判会把正在发生的阶段一起抹掉。
    let running = wire_payload("turnLifecycle", json!({ "status": "running" }));
    assert!(!is_turn_terminal(&runtime_event("wire_event", running)));

    // 其余 durable 帧与收口无关，不必每一条都去清账。
    let tool_call = wire_payload("toolCall", json!({ "partId": "tool-1" }));
    assert!(!is_turn_terminal(&runtime_event("wire_event", tool_call)));
}

/// 每次读取都前进 1 毫秒的测试时钟。
///
/// 它是「同批写入」的取证手段：一批事件共享同一个 `emitted_at_ms`，两次写入必然
/// 落到两个不同时间戳上，因此时间戳相等就等价于「只提交了一批」。
#[derive(Clone)]
struct AdvancingClock {
    next: Arc<AtomicU64>,
}

impl AdvancingClock {
    fn new(start: u64) -> Self {
        Self {
            next: Arc::new(AtomicU64::new(start)),
        }
    }
}

impl RuntimeClock for AdvancingClock {
    fn now_ms(&self) -> u64 {
        self.next.fetch_add(1, Ordering::AcqRel)
    }
}

/// 只记录投递类型的发布器：用来区分「落库」与「推实时通道」。
struct RecordingPublisher {
    published: Arc<Mutex<Vec<String>>>,
}

impl EventPublisher for RecordingPublisher {
    fn publish(&self, event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        self.published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event.event_type.clone());
        Ok(())
    }
}

#[test]
fn terminal_event_and_turn_metrics_land_in_one_batch() {
    let root = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(root.path().join("data"));
    let clock = AdvancingClock::new(1_000);
    let host_id = RuntimeHostId::new("host-turn-metrics-batch").expect("Host ID");
    let host = store
        .claim_host(&host_id, 1, None, clock.now_ms(), 60_000)
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("session-turn-metrics-batch").expect("Session ID");
    store
        .create_session(&session_id, "/workspace", clock.now_ms())
        .expect("创建会话");

    let interaction = InteractionRecord::new(
        axon_domain::DaemonSessionId::new("01J8XQ7K3M4N5P6Q7R8S9T0V1W").expect("合法会话 ID"),
        InteractionId::new("interaction-turn-metrics-batch").expect("Interaction ID"),
        InteractionSource::new("cli", "message-turn-metrics-batch").expect("来源"),
        ScopeRef::global(),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("cli", "terminal").expect("回复路由"),
        "跑一轮",
        clock.now_ms(),
    )
    .expect("Interaction");
    let interaction_id = interaction.interaction_id.clone();
    store.append(&interaction).expect("追加 Interaction");
    let lease = store
        .claim_processing_by_id(&interaction_id, clock.now_ms(), 60_000)
        .expect("领取 Interaction")
        .expect("应领取到一条待处理 Interaction");

    let published = Arc::new(Mutex::new(Vec::new()));
    let publisher = RecordingPublisher {
        published: Arc::clone(&published),
    };
    let bridge = EventStreamBridge::new();
    let sink = RuntimeTurnEventSink::new(
        &store,
        CommittedEventEgress::new(&publisher, &bridge),
        &host,
        &session_id,
        &interaction_id,
        &lease.lease_token,
        &clock,
        None,
        Arc::new(ChildSessionSinkFactory {
            store: store.clone(),
            host: host.clone(),
            clock: clock.clone(),
            stream_bridge: Arc::new(EventStreamBridge::new()),
        }),
    );

    sink.publish(&runtime_event(
        "turn_completed",
        json!({
            "interactionId": interaction_id.as_str(),
            "response": "答案",
        })
        .to_string(),
    ))
    .expect("终态事件应落库");

    // 一次写入两条：终态在前，它派生的指标紧随其后。
    let events = store.replay_events(&session_id, 0, 10).expect("回放事件");
    assert_eq!(events.len(), 2, "终态与指标必须一次写入，不得分两批");
    assert_eq!(events[0].event_type, "turn_completed");
    assert_eq!(events[1].event_type, "turn_metrics");
    assert_eq!(events[1].sequence, events[0].sequence + 1);
    assert_eq!(
        events[0].emitted_at_ms, events[1].emitted_at_ms,
        "同批事件共享同一个提交时刻：时间戳不同即说明又开了一批"
    );

    let metrics: axon_eventlog::TurnMetrics =
        serde_json::from_str(&events[1].payload_json).expect("指标载荷应可解析");
    assert_eq!(metrics.turn_id, interaction_id.as_str());

    // 指标同时进入会话累计投影：派生读数不必回放事件也能读到。
    let projected: i64 = store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM session_metrics WHERE session_id=?1",
                [session_id.as_str()],
                |row| row.get(0),
            )
        })
        .expect("读取会话累计指标");
    assert_eq!(projected, 1, "指标必须投影进 session_metrics");

    // 指标只落库：实时通道仍然只看到终态这一帧。
    assert_eq!(
        published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        ["turn_completed".to_owned()]
    );
}

#[test]
fn turn_metrics_output_tokens_follow_the_ledger_not_the_speed_window() {
    let root = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(root.path().join("data"));
    let clock = AdvancingClock::new(1_000);
    let host_id = RuntimeHostId::new("host-output-ledger").expect("Host ID");
    let host = store
        .claim_host(&host_id, 1, None, clock.now_ms(), 60_000)
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("session-output-ledger").expect("Session ID");
    store
        .create_session(&session_id, "/workspace", clock.now_ms())
        .expect("创建会话");
    let interaction = InteractionRecord::new(
        axon_domain::DaemonSessionId::new("01J8XQ7K3M4N5P6Q7R8S9T0V1W").expect("合法会话 ID"),
        InteractionId::new("interaction-output-ledger").expect("Interaction ID"),
        InteractionSource::new("cli", "message-output-ledger").expect("来源"),
        ScopeRef::global(),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("cli", "terminal").expect("回复路由"),
        "跑一轮",
        clock.now_ms(),
    )
    .expect("Interaction");
    let interaction_id = interaction.interaction_id.clone();
    store.append(&interaction).expect("追加 Interaction");
    let lease = store
        .claim_processing_by_id(&interaction_id, clock.now_ms(), 60_000)
        .expect("领取 Interaction")
        .expect("应领取到一条待处理 Interaction");
    let publisher = RecordingPublisher {
        published: Arc::new(Mutex::new(Vec::new())),
    };
    let bridge = EventStreamBridge::new();
    let sink = RuntimeTurnEventSink::new(
        &store,
        CommittedEventEgress::new(&publisher, &bridge),
        &host,
        &session_id,
        &interaction_id,
        &lease.lease_token,
        &clock,
        None,
        Arc::new(ChildSessionSinkFactory {
            store: store.clone(),
            host: host.clone(),
            clock: clock.clone(),
            stream_bridge: Arc::new(EventStreamBridge::new()),
        }),
    );

    // 两条流都在毫秒级收口：解码窗口短于速率下限，会被速率口径整条丢弃，
    // 但它们确实生成了 token，整轮读数必须算进去。
    for (output, reasoning) in [(265_u64, 1_012_u64), (40, 403)] {
        for (status, kind) in [
            ("started", "StreamingStarted"),
            ("completed", "StreamingCompleted"),
        ] {
            if status == "completed" {
                sink.publish(&runtime_event(
                    "wire_event",
                    wire_payload(
                        "usageUpdate",
                        json!({
                            "inputTokens": 100,
                            "cachedInputTokens": 900,
                            "outputTokens": output,
                            "reasoningOutputTokens": reasoning,
                        }),
                    ),
                ))
                .expect("用量帧应落库");
            }
            sink.publish(&runtime_event(
                "wire_event",
                wire_payload("turnLifecycle", json!({ "status": status, "kind": kind })),
            ))
            .expect("流边界应落库");
        }
    }
    sink.publish(&runtime_event(
        "turn_completed",
        json!({ "interactionId": interaction_id.as_str(), "response": "答案" }).to_string(),
    ))
    .expect("终态事件应落库");

    let events = store.replay_events(&session_id, 0, 64).expect("回放事件");
    let metrics_event = events
        .iter()
        .find(|event| event.event_type == "turn_metrics")
        .expect("终态应派生一条指标事件");
    let metrics: axon_eventlog::TurnMetrics =
        serde_json::from_str(&metrics_event.payload_json).expect("指标载荷应可解析");

    assert_eq!(metrics.output_tokens, 305, "短窗口流的输出也必须计入整轮");
    assert_eq!(metrics.reasoning_tokens, 1_415);
    assert_eq!(metrics.cache_read_tokens, 1_800, "五类 token 同源累加");
    // 速率只有窗口可测的流才参与：这两条流都在毫秒级收口，分母为 0，
    // 因此本轮不给速率，绝不能拿累加 token 去除一个不覆盖它们的窗口。
    assert_eq!(metrics.generation_speed_tps, None);
}

/// 用量帧按 token 逐帧投递，但持久面只留每条 provider 流的最后一条。
///
/// 真实一轮里 96% 的事件都是用量快照（同一条流把累计值推了几百上千次）。这条用例钉住
/// 两件事：实时通道确实逐帧投递（界面读数不受影响），落库只保留每条流的收口读数
/// （回放读数不丢、事件面不再被每 token 一行撑大）。
#[test]
fn usage_frames_reach_the_live_channel_but_only_the_stream_tail_is_persisted() {
    const STREAMS: usize = 3;
    const FRAMES_PER_STREAM: usize = 120;

    let root = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(root.path().join("data"));
    let clock = AdvancingClock::new(1_000);
    let host_id = RuntimeHostId::new("host-usage-coalesce").expect("Host ID");
    let host = store
        .claim_host(&host_id, 1, None, clock.now_ms(), 60_000)
        .expect("注册 Host");
    let session_id = DaemonSessionId::new("session-usage-coalesce").expect("Session ID");
    store
        .create_session(&session_id, "/workspace", clock.now_ms())
        .expect("创建会话");
    let interaction = InteractionRecord::new(
        axon_domain::DaemonSessionId::new("01J8XQ7K3M4N5P6Q7R8S9T0V1W").expect("合法会话 ID"),
        InteractionId::new("interaction-usage-coalesce").expect("Interaction ID"),
        InteractionSource::new("cli", "message-usage-coalesce").expect("来源"),
        ScopeRef::global(),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("cli", "terminal").expect("回复路由"),
        "跑一轮",
        clock.now_ms(),
    )
    .expect("Interaction");
    let interaction_id = interaction.interaction_id.clone();
    store.append(&interaction).expect("追加 Interaction");
    let lease = store
        .claim_processing_by_id(&interaction_id, clock.now_ms(), 60_000)
        .expect("领取 Interaction")
        .expect("应领取到一条待处理 Interaction");

    let published = Arc::new(Mutex::new(Vec::new()));
    let publisher = RecordingPublisher {
        published: Arc::clone(&published),
    };
    let bridge = EventStreamBridge::new();
    // 实时通道必须先订阅：桥接按订阅投递，容量要容得下这轮全部用量帧。
    let subscription = bridge
        .subscribe_session_with_capacity(session_id.clone(), None, 2_048)
        .expect("订阅实时通道");
    let sink = RuntimeTurnEventSink::new(
        &store,
        CommittedEventEgress::new(&publisher, &bridge),
        &host,
        &session_id,
        &interaction_id,
        &lease.lease_token,
        &clock,
        None,
        Arc::new(ChildSessionSinkFactory {
            store: store.clone(),
            host: host.clone(),
            clock: clock.clone(),
            stream_bridge: Arc::new(EventStreamBridge::new()),
        }),
    );

    for stream in 0..STREAMS {
        sink.publish(&runtime_event(
            "wire_event",
            wire_payload(
                "turnLifecycle",
                json!({"status": "started", "kind": "StreamingStarted"}),
            ),
        ))
        .expect("流开始应落库");
        for frame in 0..FRAMES_PER_STREAM {
            sink.publish(&runtime_event(
                "wire_event",
                wire_payload(
                    "usageUpdate",
                    json!({
                        "inputTokens": 100,
                        "outputTokens": u64::try_from(stream * FRAMES_PER_STREAM + frame + 1).expect("帧序号"),
                    }),
                ),
            ))
            .expect("用量帧应投递");
        }
        sink.publish(&runtime_event(
            "wire_event",
            wire_payload(
                "turnLifecycle",
                json!({"status": "completed", "kind": "StreamingCompleted"}),
            ),
        ))
        .expect("流收口应落库");
    }
    sink.publish(&runtime_event(
        "turn_completed",
        json!({ "interactionId": interaction_id.as_str(), "response": "答案" }).to_string(),
    ))
    .expect("终态事件应落库");

    // 实时通道：每一帧用量都投递过，界面读数不受影响。
    let mut live_usage = 0_usize;
    while let Ok(notification) = subscription.try_recv() {
        if let crate::StreamNotification::LiveEvent(event) = notification
            && event.payload_json.contains("\"usageUpdate\"")
        {
            live_usage += 1;
        }
    }
    assert_eq!(
        live_usage,
        STREAMS * FRAMES_PER_STREAM,
        "实时通道必须逐帧投递用量"
    );

    // 持久面：每条流只留最后一条。
    let events = store
        .replay_events(&session_id, 0, 1_024)
        .expect("回放事件");
    let persisted_usage = events
        .iter()
        .filter(|event| {
            event.event_type == "wire_event" && event.payload_json.contains("\"usageUpdate\"")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        persisted_usage.len(),
        STREAMS,
        "一条 provider 流只该落一条用量快照"
    );
    let tail: Value =
        serde_json::from_str(&persisted_usage[STREAMS - 1].payload_json).expect("载荷应合法");
    assert_eq!(
        tail["kind"]["payload"]["outputTokens"],
        json!(u64::try_from(STREAMS * FRAMES_PER_STREAM).expect("帧数")),
        "落库的必须是这条流的最后一条读数"
    );

    // 读数不变：整轮 token 仍按流累加，与落库条数无关。
    let metrics_event = events
        .iter()
        .find(|event| event.event_type == "turn_metrics")
        .expect("终态应派生一条指标事件");
    let metrics: axon_eventlog::TurnMetrics =
        serde_json::from_str(&metrics_event.payload_json).expect("指标载荷应可解析");
    let stream_tails = (1..=STREAMS)
        .map(|stream| stream * FRAMES_PER_STREAM)
        .sum::<usize>();
    assert_eq!(
        metrics.output_tokens,
        u64::try_from(stream_tails).expect("总输出"),
        "按流累加的整轮输出不得因收口而变"
    );
}
