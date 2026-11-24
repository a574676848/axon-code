use super::DaemonHostRecord;
use axon_domain::{DaemonSessionId, SessionActionId, SessionActionStatus};
use axon_protocol::DaemonResponse;
use axon_session::{SessionActionRecord, SessionEventRecord, SessionRuntimeRecord};

/// attach 返回的内存投影；完整历史由 cursor replay 按需读取。
#[derive(Debug, Clone)]
pub struct SessionAttachSnapshot {
    /// Session 当前状态。
    pub runtime: SessionRuntimeRecord,
    /// cursor 之后的事件，或 snapshot 的事件尾部。
    pub events: Vec<SessionEventRecord>,
    /// generation 不同，客户端必须使用 snapshot 取代局部状态。
    pub snapshot_required: bool,
}

const SNAPSHOT_CHUNK_BYTES: usize = 32 * 1024;

pub(super) fn snapshot_chunk_responses(
    request_id: String,
    body: serde_json::Value,
) -> Vec<DaemonResponse> {
    let session_id = body
        .get("runtime")
        .and_then(|runtime| runtime.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let chunks = axon_session_runtime::SnapshotChunker::new(SNAPSHOT_CHUNK_BYTES).split(&body);
    let total = u32::try_from(chunks.len()).unwrap_or(u32::MAX);
    let mut responses = Vec::with_capacity(chunks.len().saturating_add(2));
    responses.push(DaemonResponse::success(
        request_id.clone(),
        serde_json::json!({"event":"snapshot_begin","sessionId":session_id,"totalChunks":total}),
    ));
    for (index, payload) in chunks.into_iter().enumerate() {
        responses.push(DaemonResponse::success(
            request_id.clone(),
            serde_json::json!({"event":"snapshot_chunk","sessionId":session_id,"index":index,"payload":payload}),
        ));
    }
    responses.push(DaemonResponse::success(
        request_id,
        serde_json::json!({"event":"snapshot_end","sessionId":session_id,"snapshot":body}),
    ));
    responses
}

pub(super) fn attach_cursor(
    body: &serde_json::Value,
    host: Option<&DaemonHostRecord>,
) -> Option<axon_domain::ConsumerCursor> {
    let runtime = body.get("runtime")?;
    let sequence = runtime.get("lastEventSequence")?.as_u64()?;
    Some(axon_domain::ConsumerCursor::new(
        host?.generation.clone(),
        sequence,
    ))
}

pub(super) fn snapshot_to_json(snapshot: SessionAttachSnapshot) -> serde_json::Value {
    serde_json::json!({
        "runtime": snapshot.runtime,
        "events": snapshot.events,
        "snapshotRequired": snapshot.snapshot_required,
    })
}

#[must_use]
/// 创建排队 Action 的方便构造器。
pub fn queued_action(
    action_id: SessionActionId,
    session_id: DaemonSessionId,
    kind: String,
    payload_json: String,
) -> SessionActionRecord {
    SessionActionRecord {
        action_id,
        goal_id: None,
        session_id,
        kind,
        priority: 0,
        status: SessionActionStatus::Queued,
        dedupe_key: None,
        payload_json,
        goal_revision: None,
        context_version: None,
        claimed_by: None,
        claimed_generation: None,
        lease_expires_at_ms: None,
        attempt: 0,
    }
}
