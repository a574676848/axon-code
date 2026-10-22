//! Daemon Host 到 IPC 实时事件流的适配。

use super::daemon_host::{
    DEFAULT_QUEUE_LIMIT, DaemonHost, DaemonHostError, primary_session_id_for_interaction,
};
use super::event_stream_bridge::{EventStreamSubscription, StreamNotification};
use axon_domain::{
    ConsumerCursor, DaemonSessionId, InteractionId, InteractionRecord, InteractionStatus,
};
use axon_protocol::{DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonRequest, DaemonResponse};
use axon_session::{
    InteractionQueryStore, RuntimeStore, RuntimeStoreError, SessionEventRecord, UserQuestionStore,
};
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};
use std::{
    io,
    sync::mpsc::RecvTimeoutError,
    time::{Duration, Instant},
};

use crate::infrastructure::gateway_replay::event_matches_interaction;
use crate::infrastructure::ipc::{IpcDispatch, IpcResponseStream};
use crate::ports::ProcessIdentity;

pub(crate) const STREAM_RESET_EVENT: &str = "stream_reset";
pub(crate) const SUBSCRIPTION_READY_EVENT: &str = "subscription_ready";
pub(crate) const RUNTIME_OBSERVATION_EVENT: &str = "runtime_observation";
const SESSION_EVENT: &str = "session_event";
const INTERACTION_STATUS_EVENT: &str = "interaction_status";

struct OpenedEventSubscription {
    request_id: String,
    interaction_status: InteractionStatus,
    /// 已按 Interaction 过滤的 replay 页。
    events: Vec<SessionEventRecord>,
    /// 原始 replay 页是否被 `DEFAULT_QUEUE_LIMIT` 截断。
    ///
    /// 与过滤后的条数无关：页被截断时订阅方必须继续翻页，否则会漏掉自己的事件。
    page_full: bool,
    snapshot_required: bool,
    replay_cursor: Option<ConsumerCursor>,
    subscription: EventStreamSubscription,
}

struct SessionEventResponseStream {
    request_id: String,
    /// 仅用于消除“先订阅、后 replay”竞态产生的初始重叠事件。
    replay_cursor: Option<ConsumerCursor>,
    subscription: EventStreamSubscription,
    pending_response: Option<DaemonResponse>,
}

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + InteractionQueryStore + UserQuestionStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 执行普通 IPC 命令，或建立先 replay 后实时推送的事件订阅。
    pub fn handle_ipc_request(&mut self, request: DaemonRequest) -> IpcDispatch {
        let is_subscription = matches!(request.command, DaemonCommand::SubscribeEvents { .. });
        if request.protocol_version != DAEMON_PROTOCOL_VERSION || !is_subscription {
            return IpcDispatch::complete(self.handle_request(request));
        }
        let request_id = request.request_id.clone();
        let DaemonCommand::SubscribeEvents {
            session_id,
            interaction_id,
            resume_cursor,
        } = request.command
        else {
            unreachable!("订阅命令已由前置分支校验");
        };
        match self.open_event_subscription(
            request_id.clone(),
            session_id,
            interaction_id,
            resume_cursor,
        ) {
            Ok(opened) => subscription_dispatch(opened),
            Err(error) => IpcDispatch::complete(vec![error_response(request_id, error)]),
        }
    }

    fn open_event_subscription(
        &self,
        request_id: String,
        session_id: String,
        interaction_id: String,
        resume_cursor: Option<ConsumerCursor>,
    ) -> Result<OpenedEventSubscription, DaemonHostError> {
        let interaction_id = InteractionId::new(interaction_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let interaction = self
            .store_clone()
            .get_interaction(&interaction_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?
            .ok_or_else(|| DaemonHostError::Worker("订阅目标 Interaction 不存在".to_owned()))?;
        let host_generation = self.current_host_generation()?;
        // Session 身份由 Interaction 所属复合车道决定；订阅方必须与 Daemon
        // 使用同一条车道才能续读事件。
        let session_id =
            validate_subscription_target(session_id, &subscription_session_id(&interaction)?)?;

        // 新订阅只关心当前 Interaction 之后的事件。先读取 durable 尾游标，再注册
        // 实时订阅，最后按该游标回放，覆盖读取与注册之间的竞态窗口，避免从序号 0
        // 扫描整个长会话历史。
        let live_cursor = match resume_cursor.clone() {
            Some(cursor) => Some(cursor),
            None => Some(self.live_event_cursor(&session_id)?),
        };
        let subscription = self.subscribe_events(
            session_id.clone(),
            interaction_id.clone(),
            live_cursor.clone(),
        )?;
        let snapshot = match attach_from_cursor(self, &session_id, live_cursor.clone()) {
            Ok(snapshot) => Some(snapshot),
            Err(DaemonHostError::Store(RuntimeStoreError::NotFound(_))) => None,
            Err(error) => return Err(error),
        };
        let replay_cursor = snapshot
            .as_ref()
            .and_then(|value| value.events.last())
            .map(SessionEventRecord::cursor)
            .or_else(|| {
                snapshot
                    .as_ref()
                    .filter(|value| value.snapshot_required)
                    .map(|value| {
                        ConsumerCursor::new(
                            host_generation.clone(),
                            value.runtime.last_event_sequence,
                        )
                    })
            })
            .or(resume_cursor)
            .or_else(|| live_cursor.clone());
        Ok(OpenedEventSubscription {
            request_id,
            interaction_status: interaction.status,
            events: snapshot
                .as_ref()
                .map(|value| {
                    value
                        .events
                        .iter()
                        .filter(|event| event_matches_interaction(event, interaction_id.as_str()))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
            page_full: snapshot
                .as_ref()
                .is_some_and(|value| value.events.len() >= DEFAULT_QUEUE_LIMIT as usize),
            snapshot_required: snapshot.is_some_and(|value| value.snapshot_required),
            replay_cursor,
            subscription,
        })
    }
}

/// 返回复合车道对应的内部运行 Session ID。
fn subscription_session_id(
    interaction: &InteractionRecord,
) -> Result<DaemonSessionId, DaemonHostError> {
    primary_session_id_for_interaction(interaction).map_err(DaemonHostError::Worker)
}

fn validate_subscription_target(
    session_id: String,
    expected_session_id: &DaemonSessionId,
) -> Result<DaemonSessionId, DaemonHostError> {
    let session_id = DaemonSessionId::new(session_id)
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
    if &session_id != expected_session_id {
        return Err(DaemonHostError::Worker(
            "订阅的 Session 与 Interaction 所属复合车道不匹配".to_owned(),
        ));
    }
    Ok(session_id)
}

fn attach_from_cursor<S, E, P, C, I>(
    host: &DaemonHost<S, E, P, C, I>,
    session_id: &DaemonSessionId,
    resume_cursor: Option<ConsumerCursor>,
) -> Result<super::daemon_host::SessionAttachSnapshot, DaemonHostError>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    let sequence = resume_cursor.map_or(0, |cursor| cursor.sequence);
    host.attach(session_id, None, sequence, DEFAULT_QUEUE_LIMIT)
}

fn subscription_dispatch(opened: OpenedEventSubscription) -> IpcDispatch {
    let page_full = opened.page_full;
    let initial = DaemonResponse::success(
        opened.request_id.clone(),
        serde_json::json!({
            "event": SUBSCRIPTION_READY_EVENT,
            "events": opened.events,
            "pageFull": page_full,
            "snapshotRequired": opened.snapshot_required,
            "replayCursor": opened.replay_cursor,
            "interactionStatus": opened.interaction_status,
        }),
    );
    if page_full {
        return IpcDispatch::complete(vec![initial]);
    }
    IpcDispatch::Subscription {
        initial: vec![initial],
        stream: Box::new(SessionEventResponseStream {
            request_id: opened.request_id,
            replay_cursor: opened.replay_cursor,
            subscription: opened.subscription,
            pending_response: None,
        }),
    }
}

impl IpcResponseStream for SessionEventResponseStream {
    fn recv_timeout(&mut self, timeout: Duration) -> io::Result<Option<DaemonResponse>> {
        if let Some(response) = self.pending_response.take() {
            return Ok(Some(response));
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.subscription.recv_timeout(remaining) {
                Ok(StreamNotification::SessionEvent(stream_event)) => {
                    if self
                        .replay_cursor
                        .as_ref()
                        .is_some_and(|cursor| !stream_event.cursor.advances_durable_from(cursor))
                    {
                        continue;
                    }
                    self.replay_cursor = None;
                    let mut response = DaemonResponse::success(
                        self.request_id.clone(),
                        serde_json::json!({"event": SESSION_EVENT, "record": stream_event.event}),
                    );
                    response.cursor = Some(stream_event.cursor);
                    return Ok(Some(response));
                }
                Ok(StreamNotification::InteractionStatus(status)) => {
                    return Ok(Some(self.status_response(status)));
                }
                Ok(StreamNotification::Observation(frame)) => {
                    let response = DaemonResponse::success(
                        self.request_id.clone(),
                        serde_json::json!({
                            "event": RUNTIME_OBSERVATION_EVENT,
                            "frame": frame,
                        }),
                    );
                    return Ok(Some(response));
                }
                Ok(StreamNotification::StreamReset { cursor, reason }) => {
                    return Ok(Some(DaemonResponse::success(
                        self.request_id.clone(),
                        serde_json::json!({
                            "event": STREAM_RESET_EVENT,
                            "cursor": cursor,
                            "reason": reason,
                        }),
                    )));
                }
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Daemon 实时事件订阅已断开",
                    ));
                }
            }
        }
    }

    fn recv_timeout_batch(&mut self, timeout: Duration) -> io::Result<Option<Vec<DaemonResponse>>> {
        let Some(first) = self.recv_timeout(timeout)? else {
            return Ok(None);
        };
        if !is_batchable_session_event(&first) {
            return Ok(Some(vec![first]));
        }
        let mut responses = vec![first];
        while let Some(next) = self.recv_timeout(Duration::ZERO)? {
            if is_batchable_session_event(&next) {
                responses.push(next);
            } else {
                self.pending_response = Some(next);
                break;
            }
        }
        Ok(Some(responses))
    }
}

fn is_batchable_session_event(response: &DaemonResponse) -> bool {
    response.body["event"] == SESSION_EVENT && response.body["record"]["eventType"] == "wire_event"
}

impl SessionEventResponseStream {
    fn status_response(&self, status: InteractionStatus) -> DaemonResponse {
        DaemonResponse::success(
            self.request_id.clone(),
            serde_json::json!({"event": INTERACTION_STATUS_EVENT, "status": status}),
        )
    }
}

fn error_response(request_id: String, error: DaemonHostError) -> DaemonResponse {
    DaemonResponse {
        request_id,
        protocol_version: DAEMON_PROTOCOL_VERSION,
        ok: false,
        body: serde_json::json!({"error": error.to_string()}),
        cursor: None,
    }
}
#[cfg(test)]
#[path = "daemon_host_event_subscription_tests.rs"]
mod tests;
