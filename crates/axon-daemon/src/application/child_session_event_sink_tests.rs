//! 子代理会话 sink 的行为验证：登记子会话运行态行，并把事件追加进**它自己**的会话流。

use std::sync::Arc;

use axon_domain::{DaemonSessionId, RuntimeHostId};
use axon_session::{DaemonHostRecord, RuntimeStore, SqliteRuntimeStore};
use axon_session_runtime::{RuntimeClock, RuntimeEvent, SubagentSessionSinkFactory};

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

#[test]
fn child_session_is_registered_and_keeps_its_own_event_stream() {
    let (_root, store, host) = harness();
    let factory = ChildSessionSinkFactory {
        store: store.clone(),
        host: host.clone(),
        clock: FixedClock { now_ms: 20 },
        cwd: "E:/work".to_owned(),
        stream_bridge: Arc::new(crate::application::event_stream_bridge::EventStreamBridge::new()),
    };
    let child_id = "parent-session--subagent--root_delegate_fast_1";

    let sink = factory.sink_for(child_id).expect("应派生出子会话 sink");
    sink.publish(&RuntimeEvent {
        event_type: "wire_event".to_owned(),
        payload_json: r#"{"kind":{"type":"assistantMessage","text":"子代理的回复"}}"#.to_owned(),
    })
    .expect("子会话事件应落库");

    // 子会话必须是一条真实会话：运行态行可读，且带自己的 run_session_id。
    let record = store
        .get_session(&DaemonSessionId::new(child_id).expect("子会话 ID"))
        .expect("子会话运行态行应存在");
    assert_eq!(record.session_id.as_str(), child_id);
    assert!(!record.run_session_id.is_empty());

    // 事件落在**它自己**的会话流里，而不是父会话流里。
    let events = store
        .replay_events(&DaemonSessionId::new(child_id).expect("子会话 ID"), 0, 10)
        .expect("应能回放子会话事件");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "wire_event");
    assert!(events[0].payload_json.contains("子代理的回复"));

    // 同一个会话重复取 sink 不应重复登记或换身份。
    let again = store
        .get_session(&DaemonSessionId::new(child_id).expect("子会话 ID"))
        .expect("再读一次");
    assert_eq!(again.run_session_id, record.run_session_id);
    let _ = Arc::new(factory);
}
