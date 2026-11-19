//! 后台执行的投影落点：逐字稿必须落 turn 所属子会话，执行会话只留账本关系。

use std::sync::Arc;

use axon_domain::{ConsumerCursor, DaemonSessionId, RuntimeHostId};
use axon_session::{
    EventLogStore as _, RuntimeStore as _, SessionEventRecord, SessionKind, SqliteRuntimeStore,
};
use axon_session_runtime::{
    EventPublisher, RuntimeClock, RuntimeEvent, SubagentSessionIdentity,
    SubagentSessionSinkFactory, TurnEventSink,
};

use super::ExecutionSessionSinkFactory;

const NOW_MS: u64 = 1_000;
const EXECUTION_SESSION: &str = "01M7RQ9NYW8S6SQ8GTNM730M96";
const CHILD_SESSION: &str = "01M7RSKXXX8VNSK4WM3BN77R0H";
const TASK_PATH: &str = "/root/task-01m7rq9nyw8s6sq8gtnm730m96";

#[derive(Clone, Copy)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}

#[derive(Clone, Copy)]
struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

fn fixture() -> (SqliteRuntimeStore, DaemonSessionId) {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let execution_session = DaemonSessionId::new(EXECUTION_SESSION).expect("执行会话标识");
    store
        .create_session(&execution_session, "workspace", NOW_MS)
        .expect("登记执行会话");
    (store, execution_session)
}

#[test]
fn background_transcript_lands_on_the_child_session() {
    let (store, execution_session) = fixture();
    let host = store
        .claim_host(
            &RuntimeHostId::new("projection-host").expect("Host ID"),
            std::process::id(),
            None,
            NOW_MS,
            60_000,
        )
        .expect("claim host");
    let factory = ExecutionSessionSinkFactory {
        store: store.clone(),
        host,
        clock: FixedClock,
        publisher: NoopPublisher,
    };
    let identity = SubagentSessionIdentity {
        session_id: CHILD_SESSION.to_owned(),
        root_session_id: execution_session.as_str().to_owned(),
        agent_path: TASK_PATH.to_owned(),
    };
    let sink = factory.sink_for(&identity).expect("子会话 sink 必须可用");
    TurnEventSink::publish(
        sink.as_ref(),
        &RuntimeEvent {
            event_type: "assistant_text".to_owned(),
            payload_json: "{}".to_owned(),
        },
    )
    .expect("逐字稿必须写入");

    let child_id = DaemonSessionId::new(CHILD_SESSION).expect("子会话标识");
    let child_events = store
        .replay_events(&child_id, 0, 16)
        .expect("读回子会话事件");
    assert_eq!(child_events.len(), 1, "逐字稿必须落 turn 所属子会话");

    let root_events = store
        .replay_events(&execution_session, 0, 16)
        .expect("读回执行会话事件");
    assert!(
        root_events.is_empty(),
        "执行会话只做来源账本，不得承接逐字稿：实际 {:?}",
        root_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>()
    );

    let child = store.get_session(&child_id).expect("子会话必须已登记");
    assert_eq!(child.kind, SessionKind::Subagent);
    // 落库形态去掉了前导斜杠：身份比较必须以归一后的文本为准。
    assert_eq!(
        child.agent_path.as_deref(),
        Some(TASK_PATH.trim_start_matches('/'))
    );
    assert_eq!(
        child.root_session_id.as_ref().map(DaemonSessionId::as_str),
        Some(execution_session.as_str())
    );
}

#[test]
fn sink_for_rejects_an_invalid_child_session_id() {
    let (store, execution_session) = fixture();
    let host = store
        .claim_host(
            &RuntimeHostId::new("projection-host").expect("Host ID"),
            std::process::id(),
            None,
            NOW_MS,
            60_000,
        )
        .expect("claim host");
    let factory = ExecutionSessionSinkFactory {
        store,
        host,
        clock: FixedClock,
        publisher: NoopPublisher,
    };
    let identity = SubagentSessionIdentity {
        session_id: "   ".to_owned(),
        root_session_id: execution_session.as_str().to_owned(),
        agent_path: TASK_PATH.to_owned(),
    };
    assert!(
        factory.sink_for(&identity).is_err(),
        "空会话标识必须直接报错，不能静默落到别的会话"
    );
}

/// 运行侧必须自己收口子会话终态：失败轮不能让子会话历史悬空。
#[test]
fn factory_settles_child_terminal_in_the_runtime() {
    let (store, execution_session) = fixture();
    let host = store
        .claim_host(
            &RuntimeHostId::new("projection-host").expect("Host ID"),
            std::process::id(),
            None,
            NOW_MS,
            60_000,
        )
        .expect("claim host");
    let factory = Arc::new(ExecutionSessionSinkFactory {
        store,
        host,
        clock: FixedClock,
        publisher: NoopPublisher,
    });
    assert!(!factory.failures_settled_by_scheduler());
    assert_eq!(execution_session.as_str(), EXECUTION_SESSION);
}
