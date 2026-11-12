//! 执行期投影的落库契约：逐条事件由运行侧写，收口不得被追加第二遍。

use crate::application::{ExecutionProjection, ExecutionWorker};
use crate::model::{ActionExecutionRequest, RuntimeEvent, TurnExecutionOutcome};
use crate::ports::{EventPublisher, RuntimeClock, TurnEventSink, TurnExecutor};
use axon_domain::{
    ConsumerCursor, DaemonSessionId, RuntimeHostId, SessionActionId, SessionActionStatus,
};
use axon_session::{
    CheckpointTelemetry, DaemonHostRecord, RuntimeStore, SessionActionRecord, SessionEventRecord,
    SqliteRuntimeStore,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const TURN_COMPLETED: &str = "turn_completed";
const WIRE_EVENT: &str = "wire_event";

#[derive(Clone, Copy)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        100
    }
}

#[derive(Clone, Copy)]
struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

/// 把事件按宿主代际写进会话，并在写终态时置位「终态已写」。
struct SessionSink {
    store: SqliteRuntimeStore,
    host: DaemonHostRecord,
    session_id: DaemonSessionId,
    terminal_written: Arc<AtomicBool>,
}

impl TurnEventSink for SessionSink {
    fn publish(&self, event: &RuntimeEvent) -> Result<(), String> {
        self.store
            .append_events(
                &self.session_id,
                &self.host,
                &[(event.event_type.clone(), event.payload_json.clone())],
                100,
            )
            .map_err(|error| error.to_string())?;
        if event.event_type == TURN_COMPLETED {
            self.terminal_written.store(true, Ordering::Release);
        }
        Ok(())
    }
}

/// 像后台执行的运行侧那样：先写用户消息与逐条事件，最后写收口，再把收口
/// 一并放进 outcome.events（真实执行器的形状）。Worker 必须据此跳过重复追加。
struct StreamingExecutor;

impl TurnExecutor for StreamingExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, crate::model::TurnExecutionError> {
        unreachable!("投影契约只走流式通道")
    }

    fn execute_streaming(
        &self,
        _request: ActionExecutionRequest,
        sink: Option<&dyn TurnEventSink>,
    ) -> Result<TurnExecutionOutcome, crate::model::TurnExecutionError> {
        let sink = sink.expect("Worker 必须把投影交给执行器");
        for (event_type, payload) in [
            (WIRE_EVENT, r#"{"kind":{"type":"userMessage"}}"#),
            (WIRE_EVENT, r#"{"kind":{"type":"assistantMessage"}}"#),
            (TURN_COMPLETED, r#"{"response":"完成"}"#),
        ] {
            sink.publish(&RuntimeEvent {
                event_type: event_type.to_owned(),
                payload_json: payload.to_owned(),
            })
            .expect("投影落库必须成功");
        }
        Ok(TurnExecutionOutcome {
            events: vec![RuntimeEvent {
                event_type: TURN_COMPLETED.to_owned(),
                payload_json: r#"{"response":"完成"}"#.to_owned(),
            }],
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

fn action(session_id: &DaemonSessionId, action_id: &str) -> SessionActionRecord {
    SessionActionRecord {
        action_id: SessionActionId::new(action_id).expect("action id"),
        goal_id: None,
        session_id: session_id.clone(),
        kind: "turn".to_owned(),
        priority: 0,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json: r#"{"prompt":"做点事"}"#.to_owned(),
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    }
}

#[test]
fn projected_terminal_is_written_once_and_appended_once() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host_id = RuntimeHostId::new("projection-host").expect("host id");
    let host = store
        .claim_host(&host_id, 1, None, 10, 1_000)
        .expect("claim");
    let session_id = DaemonSessionId::new("goal-session-projection").expect("session");
    store
        .create_session(&session_id, "/repo", 10)
        .expect("session");
    store
        .enqueue_action(&action(&session_id, "projection-action"), 11)
        .expect("action 入队");

    let terminal_written = Arc::new(AtomicBool::new(false));
    let sink = Arc::new(SessionSink {
        store: store.clone(),
        host: host.clone(),
        session_id: session_id.clone(),
        terminal_written: Arc::clone(&terminal_written),
    });
    let projection = ExecutionProjection::new(sink, Arc::clone(&terminal_written));
    let executor = StreamingExecutor;

    ExecutionWorker::new(
        store.clone(),
        executor,
        NoopPublisher,
        FixedClock,
        host,
        session_id.clone(),
        60_000,
    )
    .with_projection(Some(projection))
    .pump_once()
    .expect("执行成功");

    let events = store.replay_events(&session_id, 0, 100).expect("重放事件");
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    assert_eq!(
        kinds,
        vec![WIRE_EVENT, WIRE_EVENT, TURN_COMPLETED],
        "逐条事件由投影写、收口只出现一次，Worker 不得重复追加"
    );
}
