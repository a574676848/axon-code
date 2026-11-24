//! Snapshot begin/chunk/end 事件流投影。

use axon_protocol::DaemonResponse;

/// 将已经序列化的 snapshot 转为三阶段响应流。
#[must_use]
pub fn stream_snapshot(
    request_id: &str,
    session_id: &str,
    snapshot: &str,
    chunk_bytes: usize,
) -> Vec<DaemonResponse> {
    let chunk_bytes = chunk_bytes.max(1);
    let chunks = snapshot.as_bytes().chunks(chunk_bytes).collect::<Vec<_>>();
    let mut responses = Vec::with_capacity(chunks.len().saturating_add(2));
    responses.push(DaemonResponse::success(
        request_id,
        serde_json::json!({"event":"snapshot_begin","sessionId":session_id,"totalChunks":chunks.len()}),
    ));
    for (index, chunk) in chunks.into_iter().enumerate() {
        responses.push(DaemonResponse::success(
            request_id,
            serde_json::json!({"event":"snapshot_chunk","sessionId":session_id,"index":index,"payload":String::from_utf8_lossy(chunk)}),
        ));
    }
    responses.push(DaemonResponse::success(
        request_id,
        serde_json::json!({"event":"snapshot_end","sessionId":session_id}),
    ));
    responses
}
