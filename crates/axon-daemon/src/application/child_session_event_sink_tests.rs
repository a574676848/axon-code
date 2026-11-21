//! 子代理会话 sink 的行为验证：它与普通会话同为「一等会话」，只是 `kind` 不同。
//!
//! 另有一组归属契约（INV-4）：子代理帧必须带派发轮次归属，否则详情页的按 Interaction
//! 订阅一帧都读不到；查不到派发行时必须拒绝落盘并计数，不得静默写成无归属帧。

use std::sync::Arc;

use axon_domain::{
    AGENT_DISPATCH_SOURCE_CHANNEL, AGENT_DISPATCH_SOURCE_PREFIX, AudienceContext, AudienceKind,
    DaemonSessionId, DisclosurePolicy, InteractionId, InteractionRecord, InteractionSource,
    ReplyRoute, RuntimeHostId, ScopeRef,
};
use axon_session::{
    DaemonHostRecord, EventLogStore, InteractionStore, RuntimeStore, SqliteRuntimeStore,
};
use axon_session_runtime::{
    RuntimeClock, RuntimeEvent, SubagentSessionIdentity, SubagentSessionSinkFactory,
};

use super::{ChildSessionSinkFactory, unattributed_subagent_writes};

/// 测试时钟：固定推进，避免依赖真实时间。
#[derive(Clone)]
struct FixedClock {
    now_ms: u64,
}

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.now_ms
    }
}

fn harness() -> (
    axon_test_support::TestTempDir,
    SqliteRuntimeStore,
    DaemonHostRecord,
) {
    let root = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(root.path().join("data"));
    let host_id = RuntimeHostId::new("host-child-sink").expect("host id");
    let host = store
        .claim_host(&host_id, 1, None, 10, 10_000)
        .expect("注册 Host");
    (root, store, host)
}

fn factory(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
) -> ChildSessionSinkFactory<SqliteRuntimeStore, FixedClock> {
    ChildSessionSinkFactory {
        store: store.clone(),
        host: host.clone(),
        clock: FixedClock { now_ms: 20 },
        stream_bridge: Arc::new(crate::application::event_stream_bridge::EventStreamBridge::new()),
    }
}

/// 父会话：子代理会话继承它的 Workspace 与受众边界。
fn parent_session(store: &SqliteRuntimeStore) -> DaemonSessionId {
    let session_id = DaemonSessionId::new("01J8Z0000000000000000000PP").expect("父会话");
    store
        .create_session_with(&axon_session::NewSessionRequest {
            workspace_id: Some("workspace-a".to_owned()),
            ..axon_session::NewSessionRequest::primary(session_id.clone(), "E:/work", 10)
        })
        .expect("登记父会话");
    session_id
}

fn child_identity(parent: &DaemonSessionId) -> SubagentSessionIdentity {
    SubagentSessionIdentity {
        session_id: "01J8Z0000000000000000000CC".to_owned(),
        root_session_id: parent.as_str().to_owned(),
        agent_path: "/root/delegate".to_owned(),
    }
}

/// 登记一条派发轮次：`inbox.session_id` 就是子会话，来源键带派发前缀。
///
/// 这是写入侧唯一的归属来源：`SubagentSessionIdentity` 不带派发交互标识，sink 只能按
/// 这一行反查。
fn seed_dispatch(
    store: &SqliteRuntimeStore,
    child: &DaemonSessionId,
    parent: &DaemonSessionId,
    invocation: &str,
) -> InteractionId {
    let interaction_id = InteractionId::new(format!("dispatch-{invocation}")).expect("派发轮次 ID");
    let record = InteractionRecord::new(
        child.clone(),
        interaction_id.clone(),
        InteractionSource::new(
            AGENT_DISPATCH_SOURCE_CHANNEL,
            format!(
                "{}{}:{invocation}",
                AGENT_DISPATCH_SOURCE_PREFIX,
                parent.as_str()
            ),
        )
        .expect("派发来源"),
        ScopeRef::workspace("workspace-a").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("desktop", "workbench-child").expect("回复路由"),
        "子代理的任务提示",
        12,
    )
    .expect("派发轮次");
    store.append(&record).expect("写入派发轮次");
    interaction_id
}

#[test]
fn child_session_is_registered_like_any_other_session_and_keeps_its_own_stream() {
    let (_root, store, host) = harness();
    let factory = factory(&store, &host);
    let parent = parent_session(&store);
    let identity = child_identity(&parent);
    let child_id = DaemonSessionId::new(identity.session_id.as_str()).expect("子会话 ID");
    seed_dispatch(&store, &child_id, &parent, "invocation-1");

    let sink = factory.sink_for(&identity).expect("应派生出子会话 sink");
    sink.publish(&RuntimeEvent {
        event_type: "wire_event".to_owned(),
        payload_json: r#"{"kind":{"type":"assistantMessage","text":"子代理的回复"}}"#.to_owned(),
    })
    .expect("子会话事件应落库");

    // 一等会话：运行态行可读，带自己的运行实例，类型与归属由派发方给出。
    let record = store.get_session(&child_id).expect("子会话运行态行应存在");
    assert_eq!(record.kind, axon_session::SessionKind::Subagent);
    assert_eq!(record.root_session_id.as_ref(), Some(&parent));
    // 代理路径在落库处归一：开头斜杠不参与身份。
    assert_eq!(record.agent_path.as_deref(), Some("root/delegate"));
    assert_eq!(
        record.workspace_id.as_deref(),
        Some("workspace-a"),
        "继承父会话工作区"
    );
    assert!(!record.current_run_id.is_empty());

    // 逐字稿落在**它自己**的会话流里，而不是父会话流里。
    let events = store
        .replay_events(&child_id, 0, 10)
        .expect("应能回放子会话事件");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "wire_event");
    assert!(events[0].payload_json.contains("子代理的回复"));
    assert!(
        store
            .replay_events(&parent, 0, 10)
            .expect("父会话可回放")
            .is_empty()
    );
}

#[test]
fn missing_child_row_is_registered_with_its_own_identity_not_as_primary() {
    let (_root, store, host) = harness();
    let factory = factory(&store, &host);
    let parent = parent_session(&store);
    let identity = child_identity(&parent);
    let child_id = DaemonSessionId::new(identity.session_id.as_str()).expect("子会话 ID");
    seed_dispatch(&store, &child_id, &parent, "invocation-2");

    // 逐字稿先到（宿主先写、派发方还没来得及登记）：这里补登记，且必须是子会话身份。
    let sink = factory.sink_for(&identity).expect("应能补登记子会话");
    sink.publish(&RuntimeEvent {
        event_type: "wire_event".to_owned(),
        payload_json: r#"{"kind":{"type":"assistantMessage","text":"先到的逐字稿"}}"#.to_owned(),
    })
    .expect("子会话事件应落库");

    let record = store.get_session(&child_id).expect("子会话运行态行应存在");
    assert_eq!(record.kind, axon_session::SessionKind::Subagent);
    assert_eq!(record.root_session_id.as_ref(), Some(&parent));
    assert_eq!(record.agent_path.as_deref(), Some("root/delegate"));
}

/// 父会话未绑定 Workspace 时，子会话同样不编造一个。
#[test]
fn child_session_without_workspace_bound_parent_stays_unbound() {
    let (_root, store, host) = harness();
    let factory = factory(&store, &host);
    let parent = DaemonSessionId::new("01J8Z0000000000000000000PG").expect("全局父会话");
    store.create_session(&parent, ".", 10).expect("登记父会话");
    let identity = SubagentSessionIdentity {
        session_id: "01J8Z0000000000000000000DD".to_owned(),
        root_session_id: parent.as_str().to_owned(),
        agent_path: "root/global".to_owned(),
    };
    factory.sink_for(&identity).expect("应能登记子会话");
    let record = store
        .get_session(&DaemonSessionId::new(identity.session_id.as_str()).expect("标识"))
        .expect("子会话运行态行应存在");
    assert!(record.workspace_id.is_none());
}

/// 子代理事件必须带派发轮次归属：详情页按 Interaction 订阅正文才有帧（INV-4 写入侧）。
#[test]
fn subagent_events_carry_dispatch_interaction_attribution() {
    let (_root, store, host) = harness();
    let factory = factory(&store, &host);
    let parent = parent_session(&store);
    let identity = child_identity(&parent);
    let child_id = DaemonSessionId::new(identity.session_id.as_str()).expect("子会话 ID");
    let dispatch = seed_dispatch(&store, &child_id, &parent, "invocation-attributed");

    let sink = factory.sink_for(&identity).expect("应派生出子会话 sink");
    for text in ["第一段", "第二段"] {
        sink.publish(&RuntimeEvent {
            event_type: "wire_event".to_owned(),
            payload_json: format!(
                "{{\"kind\":{{\"type\":\"assistantMessage\",\"text\":\"{text}\"}}}}"
            ),
        })
        .expect("子代理事件应带归属落库");
    }

    // 归属落在列上：按派发轮次取帧必须拿到这两条，而且不靠载荷里的任何字段。
    let frames = store
        .replay_interaction_events(&child_id, &dispatch, 0, 10)
        .expect("按归属回放");
    assert_eq!(frames.len(), 2, "两条逐字稿都必须挂到派发轮次下");
    assert!(frames[0].payload_json.contains("第一段"));
    assert!(frames[1].payload_json.contains("第二段"));
    // 尾窗读取同样按归属命中：桌面打开子代理详情读的就是这一条。
    let window = store
        .interaction_tail_window(&child_id, &dispatch, 10)
        .expect("尾窗读取");
    assert_eq!(window.len(), 2);

    // 归属自愈：派发行晚到时，下一次写入前重试即可补上，不再需要重建 sink。
    let late_child = DaemonSessionId::new("01J8Z0000000000000000000EE").expect("晚到的子会话");
    let late_identity = SubagentSessionIdentity {
        session_id: late_child.as_str().to_owned(),
        root_session_id: parent.as_str().to_owned(),
        agent_path: "root/late".to_owned(),
    };
    let late_sink = factory.sink_for(&late_identity).expect("派生子会话 sink");
    let late_dispatch = seed_dispatch(&store, &late_child, &parent, "invocation-late");
    late_sink
        .publish(&RuntimeEvent {
            event_type: "wire_event".to_owned(),
            payload_json: "{\"kind\":{\"type\":\"assistantMessage\",\"text\":\"晚到\"}}".to_owned(),
        })
        .expect("派发行晚到后仍应带归属落库");
    assert_eq!(
        store
            .replay_interaction_events(&late_child, &late_dispatch, 0, 10)
            .expect("按归属回放")
            .len(),
        1,
        "派发行晚到后必须自愈补上归属"
    );
}

/// 查不到派发归属时不得静默落无归属：写入被拒绝，且计数与告警留痕。
#[test]
fn subagent_events_without_dispatch_attribution_are_rejected() {
    let (_root, store, host) = harness();
    let factory = factory(&store, &host);
    let parent = parent_session(&store);
    let identity = child_identity(&parent);
    let before = unattributed_subagent_writes();

    // 没有任何派发行：这一轮是谁派的无从得知。
    let sink = factory.sink_for(&identity).expect("应派生出子会话 sink");
    let error = sink
        .publish(&RuntimeEvent {
            event_type: "wire_event".to_owned(),
            payload_json: "{\"kind\":{\"type\":\"assistantMessage\",\"text\":\"无归属\"}}"
                .to_owned(),
        })
        .expect_err("无归属的逐轮正文必须被拒绝");
    assert!(
        error.contains("wire_event") || error.contains("归属"),
        "错误必须可读且指向归属缺失：{error}"
    );
    assert_eq!(
        unattributed_subagent_writes(),
        before + 1,
        "无归属写入必须计数，不得静默"
    );
    let child_id = DaemonSessionId::new(identity.session_id.as_str()).expect("子会话 ID");
    assert!(
        store
            .replay_events(&child_id, 0, 10)
            .expect("回放")
            .is_empty(),
        "被拒绝的逐轮正文不得留下任何帧"
    );
}
