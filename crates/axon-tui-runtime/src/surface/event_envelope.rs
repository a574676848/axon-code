//! Runtime 事件 envelope。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::runtime_adapter::RuntimeAdapterSchemaVersion;

/// 当前 RuntimeEvent envelope schema 版本。
pub const RUNTIME_EVENT_SCHEMA_VERSION: RuntimeAdapterSchemaVersion =
    RuntimeAdapterSchemaVersion::new(1, 0);

static NEXT_RUNTIME_EVENT_ID: AtomicU64 = AtomicU64::new(1);

/// Runtime event 稳定事件 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeEventId(u64);

impl RuntimeEventId {
    /// 分配新的 runtime event id。
    #[must_use]
    pub fn next() -> Self {
        Self(NEXT_RUNTIME_EVENT_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// 返回原始数字 ID。
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Runtime event 关联 ID。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeCorrelationId(String);

impl RuntimeCorrelationId {
    /// 从稳定业务 key 创建 correlation id。
    #[must_use]
    pub fn from_key(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// 关联到单个事件。
    #[must_use]
    pub fn for_event(event_id: RuntimeEventId) -> Self {
        Self(format!("runtime-event-{}", event_id.get()))
    }

    /// 返回可显示 ID。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Runtime event 毫秒时间戳。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeEventTimestamp(u128);

impl RuntimeEventTimestamp {
    /// 当前时间戳。
    #[must_use]
    pub fn now() -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        Self(millis)
    }

    /// 从 Unix epoch 毫秒构造时间戳。
    #[must_use]
    pub const fn from_millis(millis: u128) -> Self {
        Self(millis)
    }

    /// 返回 Unix epoch 毫秒。
    #[must_use]
    pub const fn as_millis(self) -> u128 {
        self.0
    }
}

/// Runtime event 来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeEventSource {
    /// Axon CLI run progress projection。
    AxonCli,
    /// TUI 控制命令。
    TuiControlCommand,
    /// 后台 watcher 事件。
    Watcher,
    /// Runtime adapter 直接发射。
    RuntimeAdapter,
    /// TUI runtime submission driver。
    RuntimeSubmissionDriver,
    /// 本地 UI 中断或取消动作。
    UiControl,
}

/// 首个 IPC 事件携带的一次性客户端传输计时。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTransportTiming {
    /// Daemon 回执确认的 Primary Interaction ID。
    pub interaction_id: String,
    /// IPC submit 开始时间。
    pub ipc_submit_started_at_ms: u64,
    /// IPC submit 成功回执到达时间。
    pub ipc_submit_finished_at_ms: u64,
    /// durable append 完成的客户端观测上界。
    pub interaction_appended_at_ms: u64,
    /// 首个 IPC 事件到达客户端的时间。
    pub first_ipc_event_received_at_ms: u64,
}

/// Runtime event envelope。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEventEnvelope<T> {
    /// Envelope schema version。
    pub schema_version: RuntimeAdapterSchemaVersion,
    /// 事件 ID。
    pub event_id: RuntimeEventId,
    /// 事件发生时间。
    pub occurred_at: RuntimeEventTimestamp,
    /// 事件来源。
    pub source: RuntimeEventSource,
    /// 同一 runtime 语义链路的关联 ID。
    pub correlation_id: RuntimeCorrelationId,
    /// 导致该事件的上游事件。
    pub caused_by: Option<RuntimeEventId>,
    /// Provider 或 runtime attempt index。
    pub attempt_index: Option<usize>,
    /// 仅首个 IPC 事件携带的本地计时，不进入 wire 或 durable schema。
    pub transport_timing: Option<RuntimeTransportTiming>,
    /// 业务 payload。
    pub payload: T,
}

impl<T> RuntimeEventEnvelope<T> {
    /// 创建 runtime event envelope。
    #[must_use]
    pub fn new(source: RuntimeEventSource, payload: T) -> Self {
        let event_id = RuntimeEventId::next();
        Self {
            schema_version: RUNTIME_EVENT_SCHEMA_VERSION,
            event_id,
            occurred_at: RuntimeEventTimestamp::now(),
            source,
            correlation_id: RuntimeCorrelationId::for_event(event_id),
            caused_by: None,
            attempt_index: None,
            transport_timing: None,
            payload,
        }
    }

    /// 设置 correlation id。
    #[must_use]
    pub fn with_correlation_id(mut self, correlation_id: RuntimeCorrelationId) -> Self {
        self.correlation_id = correlation_id;
        self
    }

    /// 设置 caused_by。
    #[must_use]
    pub fn with_caused_by(mut self, caused_by: RuntimeEventId) -> Self {
        self.caused_by = Some(caused_by);
        self
    }

    /// 设置 attempt index。
    #[must_use]
    pub fn with_attempt_index(mut self, attempt_index: usize) -> Self {
        self.attempt_index = Some(attempt_index);
        self
    }

    /// 附加一次性的客户端传输计时。
    #[must_use]
    pub fn with_transport_timing(mut self, timing: RuntimeTransportTiming) -> Self {
        self.transport_timing = Some(timing);
        self
    }

    /// 转换 payload，保留 envelope 元数据。
    #[must_use]
    pub fn map_payload<U>(self, payload: U) -> RuntimeEventEnvelope<U> {
        RuntimeEventEnvelope {
            schema_version: self.schema_version,
            event_id: self.event_id,
            occurred_at: self.occurred_at,
            source: self.source,
            correlation_id: self.correlation_id,
            caused_by: self.caused_by,
            attempt_index: self.attempt_index,
            transport_timing: self.transport_timing,
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_payload_preserves_transport_timing() {
        let timing = RuntimeTransportTiming {
            interaction_id: "interaction-1".to_owned(),
            ipc_submit_started_at_ms: 10,
            ipc_submit_finished_at_ms: 20,
            interaction_appended_at_ms: 20,
            first_ipc_event_received_at_ms: 30,
        };
        let event = RuntimeEventEnvelope::new(RuntimeEventSource::AxonCli, "before")
            .with_transport_timing(timing.clone())
            .map_payload("after");

        assert_eq!(event.transport_timing, Some(timing));
        assert_eq!(event.payload, "after");
    }
}
