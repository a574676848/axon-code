//! 子代理会话 sink 的行为验证：它与普通会话同为「一等会话」，只是 `kind` 不同。

use std::sync::Arc;

use axon_domain::{DaemonSessionId, RuntimeHostId};
use axon_session::{DaemonHostRecord, EventLogStore, RuntimeStore, SqliteRuntimeStore};
use axon_session_runtime::{
    RuntimeClock, RuntimeEvent, SubagentSessionIdentity, SubagentSessionSinkFactory,
};

use super::ChildSessionSinkFactory;

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

#[test]
fn child_session_is_registered_like_any_other_session_and_keeps_its_own_stream() {
    let (_root, store, host) = harness();
    let factory = factory(&store, &host);
    let parent = parent_session(&store);
    let identity = child_identity(&parent);
    let child_id = DaemonSessionId::new(identity.session_id.as_str()).expect("子会话 ID");

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

    // 逐字稿先到（宿主先写、派发方还没来得及登记）：这里补登记，且必须是子会话身份。
    let sink = factory.sink_for(&identity).expect("应能补登记子会话");
    sink.publish(&RuntimeEvent {
        event_type: "wire_event".to_owned(),
        payload_json: r#"{"kind":{"type":"assistantMessage","text":"先到的逐字稿"}}"#.to_owned(),
    })
    .expect("子会话事件应落库");

    let child_id = DaemonSessionId::new(identity.session_id.as_str()).expect("子会话 ID");
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
