//! 远控事件游标与有界 replay。
//!
//! 该模块借鉴 OpenClaw event hub 的 replay window 与 Axon runtime gateway 的事件分发边界，
//! 但只实现本地纯 runtime 合同：调用方必须传入已结构化的 bounded event projection，
//! 本模块不监听网络、不读取原始事件流，也不拼接 session transcript。

use axon_domain::{AxonResult, RemoteAction, RemoteCapabilityPolicy, SessionId};

/// 远控事件窗口最多返回事件数。
pub const REMOTE_EVENT_CURSOR_MAX_WINDOW_EVENTS: usize = 32;
/// 单个远控事件 payload 最大字符数。
pub const REMOTE_EVENT_CURSOR_MAX_PAYLOAD_CHARS: usize = 240;
/// 远控事件 cursor 默认 TTL。
pub const REMOTE_EVENT_CURSOR_TTL_MS: u64 = 120_000;
/// 远控事件 cursor 窗口已投影原因。
pub const REMOTE_EVENT_CURSOR_REASON_WINDOW_PROJECTED: &str =
    "remote_event_cursor:window_projected";
/// 远控事件 cursor 关闭完整 replay 原因。
pub const REMOTE_EVENT_CURSOR_REASON_FULL_REPLAY_CLOSED: &str =
    "remote_event_cursor:full_replay_closed";

/// 远控事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteEventKind {
    /// 状态摘要。
    StatusSummary,
    /// 通知摘要。
    Notification,
    /// 单个 `OutputRef` grant 摘要。
    OutputRefGrant,
    /// 上传入口摘要。
    UploadIngress,
}

/// 远控事件投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEventProjection {
    /// 单调递增序号。
    pub sequence: u64,
    /// 目标 session。
    pub session_id: SessionId,
    /// 事件类型。
    pub kind: RemoteEventKind,
    /// 有界 payload。
    pub bounded_payload: String,
    /// 原始 payload 字符数。
    pub payload_chars: usize,
    /// 是否已截断。
    pub truncated: bool,
}

impl RemoteEventProjection {
    /// 从摘要文本生成有界事件投影。
    #[must_use]
    pub fn bounded(
        sequence: u64,
        session_id: SessionId,
        kind: RemoteEventKind,
        payload: &str,
    ) -> Self {
        let payload_chars = payload.chars().count();
        let bounded_payload = payload
            .chars()
            .take(REMOTE_EVENT_CURSOR_MAX_PAYLOAD_CHARS)
            .collect::<String>();
        Self {
            sequence,
            session_id,
            kind,
            bounded_payload,
            payload_chars,
            truncated: payload_chars > REMOTE_EVENT_CURSOR_MAX_PAYLOAD_CHARS,
        }
    }
}

/// 远控事件游标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteEventCursor {
    /// 下一条待读取序号。
    pub next_sequence: u64,
    /// 发放时间。
    pub issued_at_ms: u64,
    /// 过期时间。
    pub expires_at_ms: u64,
}

/// 远控事件读取输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEventCursorReadInput {
    /// 目标 session。
    pub session_id: SessionId,
    /// 已持有 cursor；为空时从可用窗口头部开始。
    pub cursor: Option<RemoteEventCursor>,
    /// 当前时间。
    pub now_ms: u64,
    /// 可用事件窗口。
    pub events: Vec<RemoteEventProjection>,
    /// 请求读取数量。
    pub requested_limit: usize,
}

/// 远控事件读取决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteEventCursorReadDecision {
    /// 返回有界窗口。
    Window(RemoteEventWindow),
    /// 拒绝读取。
    Blocked(RemoteEventCursorBlockReason),
}

/// 远控事件窗口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEventWindow {
    /// 下一次读取使用的 cursor。
    pub cursor: RemoteEventCursor,
    /// 有界事件。
    pub events: Vec<RemoteEventProjection>,
    /// 是否因请求数量或可用窗口被截断。
    pub truncated: bool,
    /// 是否允许完整 replay。
    pub full_replay_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控事件 cursor 阻断原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteEventCursorBlockReason {
    /// cursor 已过期。
    CursorExpired,
    /// cursor 落后于可用窗口，慢消费者必须重新获取 bounded snapshot。
    SlowConsumer,
    /// 事件 payload 未保持有界。
    PayloadTooLarge,
    /// 请求窗口无效。
    InvalidWindowLimit,
}

/// 远控事件 cursor runtime。
#[derive(Debug, Clone)]
pub struct RemoteEventCursorRuntime {
    policy: RemoteCapabilityPolicy,
}

impl RemoteEventCursorRuntime {
    /// 创建 runtime。
    #[must_use]
    pub const fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self { policy }
    }

    /// 读取远控事件窗口。
    ///
    /// # Errors
    ///
    /// 当远控策略不允许 bounded summary 时返回错误。
    pub fn read_window(
        &self,
        input: &RemoteEventCursorReadInput,
    ) -> AxonResult<RemoteEventCursorReadDecision> {
        self.policy
            .ensure_allowed(RemoteAction::ReadBoundedSummary)?;

        if input.requested_limit == 0
            || input.requested_limit > REMOTE_EVENT_CURSOR_MAX_WINDOW_EVENTS
        {
            return Ok(RemoteEventCursorReadDecision::Blocked(
                RemoteEventCursorBlockReason::InvalidWindowLimit,
            ));
        }
        if input.events.iter().any(payload_exceeds_boundary) {
            return Ok(RemoteEventCursorReadDecision::Blocked(
                RemoteEventCursorBlockReason::PayloadTooLarge,
            ));
        }
        if input
            .cursor
            .is_some_and(|cursor| input.now_ms > cursor.expires_at_ms)
        {
            return Ok(RemoteEventCursorReadDecision::Blocked(
                RemoteEventCursorBlockReason::CursorExpired,
            ));
        }

        let mut session_events = input
            .events
            .iter()
            .filter(|event| event.session_id == input.session_id)
            .cloned()
            .collect::<Vec<_>>();
        session_events.sort_by_key(|event| event.sequence);

        let first_available = session_events.first().map(|event| event.sequence);
        let start_sequence = input.cursor.map_or_else(
            || first_available.unwrap_or(0),
            |cursor| cursor.next_sequence,
        );
        if first_available.is_some_and(|first| start_sequence < first) {
            return Ok(RemoteEventCursorReadDecision::Blocked(
                RemoteEventCursorBlockReason::SlowConsumer,
            ));
        }

        let matching = session_events
            .into_iter()
            .filter(|event| event.sequence >= start_sequence)
            .collect::<Vec<_>>();
        let events = matching
            .iter()
            .take(input.requested_limit)
            .cloned()
            .collect::<Vec<_>>();
        let next_sequence = events
            .last()
            .map_or(start_sequence, |event| event.sequence.saturating_add(1));
        let cursor = RemoteEventCursor {
            next_sequence,
            issued_at_ms: input.now_ms,
            expires_at_ms: input.now_ms.saturating_add(REMOTE_EVENT_CURSOR_TTL_MS),
        };

        Ok(RemoteEventCursorReadDecision::Window(RemoteEventWindow {
            cursor,
            truncated: matching.len() > events.len(),
            events,
            full_replay_allowed: false,
            reasons: vec![
                REMOTE_EVENT_CURSOR_REASON_WINDOW_PROJECTED.to_owned(),
                REMOTE_EVENT_CURSOR_REASON_FULL_REPLAY_CLOSED.to_owned(),
            ],
        }))
    }
}

fn payload_exceeds_boundary(event: &RemoteEventProjection) -> bool {
    event.bounded_payload.chars().count() > REMOTE_EVENT_CURSOR_MAX_PAYLOAD_CHARS
}
