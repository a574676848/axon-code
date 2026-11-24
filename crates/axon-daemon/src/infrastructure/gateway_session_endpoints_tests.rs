//! 会话级固定接口的路由契约：快照读取与静默轮次显式收口。
//!
//! 走真实 socket 与真实 `DaemonGatewayBridge`：这里锁的是车道 G 消费的线上契约
//! （状态码、字段名、可空性）与收口的幂等语义，而不是 handler 内部实现。

use std::sync::Arc;

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, InteractionStatus, ReplyRoute, RuntimeHostId, ScopeRef,
};
use axon_session::{
    EventLogStore, InteractionQueryStore, InteractionStore, RuntimeStore, SqliteRuntimeStore,
};
use axon_session_runtime::RuntimeClock;
use serde_json::{Value, json};

use super::super::super::test_support::{BridgeServer, TEST_AUTH_TOKEN, get, post, send_raw};
use crate::application::{EventStreamBridge, GatewayHostDependencies, NoActiveTurnSteerSink};
use crate::infrastructure::gateway_bridge::DaemonGatewayBridge;
use crate::infrastructure::gateway_kernel::GatewayIngress;

/// 固定时钟：判定与租约都不依赖真实时间。
const NOW_MS: u64 = 1_760_000_000_000;
const ACTION_LEASE_DURATION_MS: u64 = 120_000;
const SNAPSHOT_INTERACTION_ID: &str = "interaction-snapshot-route";

#[derive(Clone)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}

struct Fixture {
    store: SqliteRuntimeStore,
    server: BridgeServer,
    // 字段顺序即析构顺序：临时目录最后析构，先释放服务线程与 SQLite 句柄。
    _temp: axon_test_support::TestTempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::from_root(temp.path().join("data"));
        store.initialize().expect("初始化运行态");
        let bridge = DaemonGatewayBridge::new(GatewayHostDependencies {
            store: store.clone(),
            active_turn_steer_sink: Arc::new(NoActiveTurnSteerSink),
            clock: FixedClock,
            action_lease_duration_ms: ACTION_LEASE_DURATION_MS,
            stream_bridge: Arc::new(EventStreamBridge::new()),
            host_generation: None,
        });
        let server = BridgeServer::start(Arc::new(bridge) as Arc<dyn GatewayIngress>);
        Self {
            store,
            server,
            _temp: temp,
        }
    }

    fn session(&self, raw: &str) -> DaemonSessionId {
        let session_id = DaemonSessionId::new(raw).expect("Session ID");
        self.store
            .create_session(&session_id, "/workspace", NOW_MS)
            .expect("登记会话");
        session_id
    }

    fn request(&self, raw: &str) -> (u16, String) {
        send_raw(self.server.address, raw)
    }

    fn get_json(&self, target: &str) -> (u16, Value) {
        let (status, body) = self.request(&get(target, Some(TEST_AUTH_TOKEN)));
        let value = serde_json::from_str(&body).unwrap_or(Value::Null);
        (status, value)
    }

    fn post_json(&self, target: &str, payload: &Value) -> (u16, Value) {
        let body = payload.to_string();
        let (status, response) = self.request(&post(
            target,
            Some(TEST_AUTH_TOKEN),
            Some("application/json"),
            &body,
        ));
        let value = serde_json::from_str(&response).unwrap_or(Value::Null);
        (status, value)
    }
}

#[test]
fn context_snapshot_route_returns_null_snapshot_for_empty_session() {
    let fixture = Fixture::new();
    let session_id = fixture.session("session-context-route");
    let target = format!("/api/v1/sessions/{}/context-snapshot", session_id.as_str());

    let (status, payload) = fixture.get_json(&target);
    assert_eq!(status, 200, "已有会话必须 200：{payload}");
    assert_eq!(payload["sessionId"], session_id.as_str());
    assert_eq!(
        payload["snapshot"],
        Value::Null,
        "还没有任何读数时 snapshot 必须是 null，不能是全空对象"
    );

    // 未知会话：404 且错误体与既有扩展路由同形（error.code / message / details）。
    let (missing_status, missing) =
        fixture.get_json("/api/v1/sessions/session-not-registered/context-snapshot");
    assert_eq!(missing_status, 404, "{missing}");
    assert_eq!(missing["error"]["code"], "SESSION_NOT_FOUND");
    assert_eq!(missing["error"]["details"], Value::Null);
    assert!(
        missing["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );

    // 未带凭据：与既有路由同口径的 401，不泄露会话是否存在。
    let (unauthorized_status, _) = fixture.request(&get(&target, None));
    assert_eq!(unauthorized_status, 401);
}

#[test]
fn context_snapshot_route_projects_reading_with_gateway_field_names() {
    let fixture = Fixture::new();
    let session_id = fixture.session("session-context-reading");
    let host = fixture
        .store
        .claim_host(
            &RuntimeHostId::new("route-contract-host").expect("Host ID"),
            1,
            None,
            NOW_MS,
            60_000,
        )
        .expect("领取 Host lease");
    let interaction_id = InteractionId::new(SNAPSHOT_INTERACTION_ID).expect("Interaction ID");
    let frame = (
        "wire_event".to_owned(),
        json!({
            "recordedAtMs": NOW_MS,
            "interactionId": SNAPSHOT_INTERACTION_ID,
            "kind": {
                "type": "usageUpdate",
                "payload": {
                    "currentContextTokens": 135_917,
                    "contextWindow": 262_144,
                    "conversationTokens": 80_821,
                },
            },
        })
        .to_string(),
    );
    fixture
        .store
        .append_attributed_events(&session_id, &host, &interaction_id, &[frame], NOW_MS + 1)
        .expect("写入用量帧");

    let (status, payload) = fixture.get_json(&format!(
        "/api/v1/sessions/{}/context-snapshot",
        session_id.as_str()
    ));
    assert_eq!(status, 200, "{payload}");
    let snapshot = &payload["snapshot"];
    assert_eq!(snapshot["currentContextTokens"], 135_917);
    assert_eq!(snapshot["contextWindow"], 262_144);
    assert_eq!(snapshot["composition"]["conversationTokens"], 80_821);
    assert_eq!(snapshot["updatedAtMs"], NOW_MS);
    assert_eq!(snapshot["source"], "usage_frame");
    // 网关只发原始读数：剩余量与换窗前后读数都不在线上契约里。
    assert!(snapshot.get("remainingTokens").is_none());
    assert!(snapshot.get("rolloverBeforeTokens").is_none());
    assert!(snapshot.get("rolloverAfterTokens").is_none());

    // 不带窗口的用量帧：窗口保持未知，同样不得冒出剩余量字段。
    let no_window_frame = (
        "wire_event".to_owned(),
        json!({
            "recordedAtMs": NOW_MS + 5,
            "interactionId": SNAPSHOT_INTERACTION_ID,
            "kind": {
                "type": "usageUpdate",
                "payload": { "currentContextTokens": 90_000 },
            },
        })
        .to_string(),
    );
    fixture
        .store
        .append_attributed_events(
            &session_id,
            &host,
            &interaction_id,
            &[no_window_frame],
            NOW_MS + 6,
        )
        .expect("写入不带窗口的用量帧");
    let (_, payload) = fixture.get_json(&format!(
        "/api/v1/sessions/{}/context-snapshot",
        session_id.as_str()
    ));
    let snapshot = &payload["snapshot"];
    assert_eq!(snapshot["currentContextTokens"], 90_000);
    assert!(snapshot["contextWindow"].is_null(), "窗口缺失就是未知");
    assert!(
        snapshot.get("remainingTokens").is_none(),
        "没有窗口就不派生剩余量"
    );
}

#[test]
fn settle_silent_turn_route_is_idempotent() {
    let fixture = Fixture::new();
    let session_id = fixture.session("session-settle-route");
    let interaction = interaction_record(&session_id, "interaction-settle-route");
    fixture.store.append(&interaction).expect("写入入站事实");
    // 领取时租约只有 100ms：到固定时钟这一刻早已过期，这一轮没有任何执行者。
    fixture
        .store
        .claim_processing_by_id(&interaction.interaction_id, NOW_MS - 1_000, 100)
        .expect("领取")
        .expect("租约");
    let target = format!(
        "/api/v1/sessions/{}/settle-silent-turn",
        session_id.as_str()
    );
    let body = json!({ "interactionId": interaction.interaction_id.as_str() });

    let (status, receipt) = fixture.post_json(&target, &body);
    assert_eq!(status, 200, "收口是业务结论，不是错误：{receipt}");
    assert_eq!(receipt["settled"], true);
    assert_eq!(receipt["verdict"], "lease_expired");
    assert_eq!(
        turn_status(
            &fixture.store,
            &session_id,
            interaction.interaction_id.as_str()
        ),
        InteractionStatus::Interrupted,
        "收口后索引必须读得出终态"
    );
    assert_eq!(
        ledger_event_count(&fixture.store, "interaction.interrupted"),
        1,
        "收口必须留下既有事件名的流水账"
    );

    // 重复调用：没有在途轮次时如实回 settled=false 与判据，不报错、不重复写终态。
    let (again_status, again) = fixture.post_json(&target, &body);
    assert_eq!(again_status, 200, "{again}");
    assert_eq!(again["settled"], false);
    assert_eq!(again["verdict"], "no_in_flight_turn");
    assert_eq!(
        ledger_event_count(&fixture.store, "interaction.interrupted"),
        1,
        "幂等由存储层的状态守卫保证"
    );

    // 请求体缺字段：400，不把畸形请求当成「没有在途轮次」。
    let (invalid_status, _) = fixture.post_json(&target, &json!({}));
    assert_eq!(invalid_status, 400);
}

#[test]
fn live_payload_marks_stale_lease_as_not_running() {
    let fixture = Fixture::new();

    // 一、会话存在但没有任何在跑轮次：running 必须为假，租约余量留空而不是补零。
    let idle = fixture.session("session-live-idle");
    let idle_activity = fixture
        .store
        .get_session(&idle)
        .expect("会话行")
        .last_activity_at_ms;
    let (status, idle_payload) =
        fixture.get_json(&format!("/api/v1/sessions/{}/live", idle.as_str()));
    assert_eq!(status, 200, "已有会话必须 200：{idle_payload}");
    assert_eq!(idle_payload["sessionId"], idle.as_str());
    assert_eq!(idle_payload["running"], false, "没有在跑轮次时不能说在跑");
    assert_eq!(
        idle_payload["leaseRemainingMs"],
        Value::Null,
        "没有租约时留空，不写 0"
    );
    assert_eq!(
        idle_payload["idleForMs"].as_u64(),
        Some(NOW_MS - idle_activity),
        "静默时长取最后活动时刻到判定时刻的距离"
    );
    assert_eq!(
        idle_payload["lastFrameSequence"].as_u64(),
        Some(0),
        "从未写过事件的会话尾序号为 0"
    );
    assert_eq!(
        idle_payload["queuedTurns"].as_u64(),
        Some(0),
        "没有排队轮次时如实报零"
    );

    // 二、有在跑轮次但租约已过期：没有执行者，证据不成立，running 仍必须是假。
    let stale = fixture.session("session-live-stale");
    let stale_turn = interaction_record(&stale, "interaction-live-stale");
    fixture.store.append(&stale_turn).expect("写入入站事实");
    fixture
        .store
        .claim_processing_by_id(&stale_turn.interaction_id, NOW_MS - 1_000, 100)
        .expect("领取")
        .expect("租约");
    let (stale_status, stale_payload) =
        fixture.get_json(&format!("/api/v1/sessions/{}/live", stale.as_str()));
    assert_eq!(stale_status, 200, "{stale_payload}");
    assert_eq!(
        stale_payload["running"], false,
        "租约过期即没有执行者，不得继续报在跑"
    );
    assert_eq!(stale_payload["leaseRemainingMs"], Value::Null);

    // 三、租约未过期：这才是「在跑」，余量按判定时刻实算。
    let fresh = fixture.session("session-live-fresh");
    let fresh_turn = interaction_record(&fresh, "interaction-live-fresh");
    fixture.store.append(&fresh_turn).expect("写入入站事实");
    fixture
        .store
        .claim_processing_by_id(&fresh_turn.interaction_id, NOW_MS, ACTION_LEASE_DURATION_MS)
        .expect("领取")
        .expect("租约");
    let (fresh_status, fresh_payload) =
        fixture.get_json(&format!("/api/v1/sessions/{}/live", fresh.as_str()));
    assert_eq!(fresh_status, 200, "{fresh_payload}");
    assert_eq!(fresh_payload["running"], true, "带租约且未过期才是在跑");
    assert_eq!(
        fresh_payload["leaseRemainingMs"].as_u64(),
        Some(ACTION_LEASE_DURATION_MS)
    );

    // 四、刚提交、还没被领取的排队轮次：没有租约，但它是活的，必须单独报出来。
    let queued = fixture.session("session-live-queued");
    let queued_turn = interaction_record(&queued, "interaction-live-queued");
    fixture.store.append(&queued_turn).expect("写入入站事实");
    let queued_lease = fixture
        .store
        .claim_processing_by_id(
            &queued_turn.interaction_id,
            NOW_MS,
            ACTION_LEASE_DURATION_MS,
        )
        .expect("领取")
        .expect("租约");
    fixture
        .store
        .release_processing(
            &queued_turn.interaction_id,
            &queued_lease.lease_token,
            NOW_MS,
        )
        .expect("释放回排队");
    let (queued_status, queued_payload) =
        fixture.get_json(&format!("/api/v1/sessions/{}/live", queued.as_str()));
    assert_eq!(queued_status, 200, "{queued_payload}");
    assert_eq!(
        queued_payload["running"], false,
        "还没被领取的轮次没有租约，不得报在跑"
    );
    assert_eq!(
        queued_payload["queuedTurns"].as_u64(),
        Some(1),
        "排队轮次必须单独报出来：读侧据此区分「还没开始」与「已经没有执行者」"
    );

    // 未知会话 404，未带凭据 401：与同一路由族的其余接口同口径。
    let (missing_status, missing) =
        fixture.get_json("/api/v1/sessions/session-not-registered/live");
    assert_eq!(missing_status, 404, "{missing}");
    assert_eq!(missing["error"]["code"], "SESSION_NOT_FOUND");
    let (unauthorized_status, _) = fixture.request(&get(
        &format!("/api/v1/sessions/{}/live", idle.as_str()),
        None,
    ));
    assert_eq!(unauthorized_status, 401);
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
        NOW_MS - 2_000,
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

fn ledger_event_count(store: &SqliteRuntimeStore, event_type: &str) -> i64 {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM ledger_events WHERE event_type=?1",
                [event_type],
                |row| row.get(0),
            )
        })
        .expect("流水账计数")
}
