//! 轮次头订阅：只报「多了一轮」与「这一轮收口了」，不重放历史。

use std::time::Duration;

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};
use axon_session::{InteractionStore as _, RuntimeStore as _, SqliteRuntimeStore};

use super::{INTERACTION_CREATED_EVENT, INTERACTION_SETTLED_EVENT, SessionTurnsStream};
use crate::infrastructure::gateway_kernel::{GatewayEvent, GatewayEventStream};

const WAIT: Duration = Duration::from_secs(2);

struct Fixture {
    store: SqliteRuntimeStore,
    session_id: DaemonSessionId,
}

impl Fixture {
    fn new() -> Self {
        let directory = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        let session_id = axon_session::new_session_id();
        store.create_session(&session_id, ".", 1).expect("会话行");
        Self { store, session_id }
    }

    /// 落一条派发轮：渠道即派发标记，它可以不带 lease 直接结算。
    fn dispatch(&self, message: &str, received_at_ms: u64) -> InteractionRecord {
        let source = InteractionSource::new(
            axon_domain::AGENT_DISPATCH_SOURCE_CHANNEL,
            format!("agent-dispatch:{}:{message}", self.session_id.as_str()),
        )
        .expect("派发来源");
        let record = InteractionRecord::new(
            self.session_id.clone(),
            InteractionId::new(format!("interaction-{message}")).expect("交互标识"),
            source,
            ScopeRef::workspace("ws-turns").expect("作用域"),
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            ReplyRoute::new("agent", "transcript").expect("路由"),
            "跑一轮",
            received_at_ms,
        )
        .expect("交互记录");
        self.store.append(&record).expect("交互入库");
        record
    }

    fn stream(&self) -> SessionTurnsStream<SqliteRuntimeStore> {
        SessionTurnsStream::new(self.store.clone(), self.session_id.clone())
    }
}

fn event_type(event: &GatewayEvent) -> &str {
    event.event_type.as_str()
}

fn payload_interaction_id(event: &GatewayEvent) -> String {
    let payload: serde_json::Value =
        serde_json::from_str(&event.payload_json).expect("帧载荷必须是 JSON");
    payload["interactionId"]
        .as_str()
        .expect("帧必须带 interactionId")
        .to_owned()
}

#[test]
fn new_turn_is_reported_once_and_history_is_not_replayed() {
    let fixture = Fixture::new();
    let existing = fixture.dispatch("existing", 10);
    let mut stream = fixture.stream();
    // 首次读取只建立基线：已有一轮不得作为「新轮」补发。
    assert!(
        stream
            .recv_timeout(Duration::from_millis(50))
            .expect("首读")
            .is_none(),
        "基线阶段不得补发历史轮次"
    );

    let fresh = fixture.dispatch("fresh", 20);
    let event = stream
        .recv_timeout(WAIT)
        .expect("轮询应成功")
        .expect("必须报出新轮");
    assert_eq!(event_type(&event), INTERACTION_CREATED_EVENT);
    assert_eq!(
        payload_interaction_id(&event),
        fresh.interaction_id.as_str()
    );
    assert!(!event.replay, "轮次头不是历史回放");

    // 同一轮只报一次。
    fixture.dispatch("another", 30);
    let next = stream
        .recv_timeout(WAIT)
        .expect("轮询应成功")
        .expect("必须报出下一轮");
    assert_eq!(
        payload_interaction_id(&next),
        "interaction-another",
        "重复轮询不得把已经报过的新轮再报一次"
    );
    assert_ne!(
        payload_interaction_id(&next),
        existing.interaction_id.as_str()
    );
}

#[test]
fn turn_settling_within_one_poll_reports_created_then_settled() {
    let fixture = Fixture::new();
    let mut stream = fixture.stream();
    assert!(
        stream
            .recv_timeout(Duration::from_millis(50))
            .expect("首读")
            .is_none(),
        "基线阶段不得补发历史轮次"
    );

    // 一轮在两个节拍之间跑完：订阅者必须先看到它存在，再看到它收口。
    let turn = fixture.dispatch("fast", 20);
    fixture
        .store
        .settle_dispatch(
            &turn.interaction_id,
            axon_session::DispatchTerminalKind::Understood,
            30,
        )
        .expect("结算派发轮");

    let created = stream
        .recv_timeout(WAIT)
        .expect("轮询应成功")
        .expect("必须先报新轮");
    assert_eq!(event_type(&created), INTERACTION_CREATED_EVENT);
    assert_eq!(
        payload_interaction_id(&created),
        turn.interaction_id.as_str()
    );
    let settled = stream
        .recv_timeout(WAIT)
        .expect("轮询应成功")
        .expect("随后必须报收口");
    assert_eq!(event_type(&settled), INTERACTION_SETTLED_EVENT);
    assert_eq!(
        payload_interaction_id(&settled),
        turn.interaction_id.as_str()
    );
}
#[test]
fn settled_turn_is_reported_with_its_terminal_status() {
    let fixture = Fixture::new();
    let turn = fixture.dispatch("manual", 10);
    let mut stream = fixture.stream();
    assert!(
        stream
            .recv_timeout(Duration::from_millis(50))
            .expect("首读")
            .is_none()
    );

    fixture
        .store
        .settle_dispatch(
            &turn.interaction_id,
            axon_session::DispatchTerminalKind::Understood,
            30,
        )
        .expect("结算派发轮");

    let event = stream
        .recv_timeout(WAIT)
        .expect("轮询应成功")
        .expect("必须报出收口");
    assert_eq!(event_type(&event), INTERACTION_SETTLED_EVENT);
    assert_eq!(payload_interaction_id(&event), turn.interaction_id.as_str());
    let payload: serde_json::Value = serde_json::from_str(&event.payload_json).expect("载荷");
    assert_eq!(payload["status"], "understood");
}

#[test]
fn subscription_prefix_is_disjoint_from_session_and_interaction_prefixes() {
    use super::session_turns_target;
    assert_eq!(session_turns_target("session-turns:01JABC"), Some("01JABC"));
    assert_eq!(session_turns_target("session:01JABC"), None);
    assert_eq!(session_turns_target("interaction:01JABC"), None);
    assert_eq!(session_turns_target("session-turns:"), None);
}
