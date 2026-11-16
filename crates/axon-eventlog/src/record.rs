//! 事件记录模型。

use serde::{Deserialize, Serialize};

use crate::error::EventLogError;

/// 事件的来源类别。
///
/// 渠道属于轮次而不属于会话：同一会话可以先后由桌面与 TUI 产生不同来源的轮次。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOrigin {
    /// 外部渠道发起（桌面、TUI、CLI、飞书等）。
    #[default]
    External,
    /// 定时任务触发。
    Cron,
    /// 长期目标推进。
    Goal,
    /// 子代理发起。
    Subagent,
}

/// 单轮可观测指标。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnMetrics {
    /// 轮次标识。
    pub turn_id: String,
    /// 首字延迟；`None` 表示未测得，禁止用 0 冒充。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    /// 本轮墙钟耗时。
    pub active_ms: u64,
    /// 模型往返耗时。
    pub provider_duration_ms: u64,
    /// 工具执行耗时。
    pub tool_ms: u64,
    /// 输入 token。
    pub input_tokens: u64,
    /// 输出 token。
    pub output_tokens: u64,
    /// 推理 token。
    pub reasoning_tokens: u64,
    /// 缓存读取 token。
    pub cache_read_tokens: u64,
    /// 缓存写入 token。
    pub cache_write_tokens: u64,
    /// 生成速率（tokens/秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_speed_tps: Option<f64>,
    /// 本轮启动的后台任务数。
    pub background_jobs_started: u32,
    /// 收尾时仍在运行的后台任务数。
    pub background_jobs_running: u32,
}

/// 事件类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKind {
    /// 一轮开始。
    TurnStarted,
    /// 一轮结束。
    TurnCompleted,
    /// 工具调用开始。
    ToolCall,
    /// 工具调用结束。
    ToolResult,
    /// assistant 正文快照。
    AssistantMessage,
    /// Interaction 状态变化。
    InteractionState,
    /// Interaction 被中断。
    InteractionInterrupted,
    /// 检查点。
    Checkpoint,
    /// 崩溃恢复标记。
    SessionRecovered,
    /// 单轮指标。
    TurnMetrics,
    /// 后台任务开始。
    JobStarted,
    /// 后台任务结束。
    JobCompleted,
    /// 尚未收敛成强类型的既有事件类型，保留原始名称。
    Other(String),
}

impl RuntimeEventKind {
    /// 把事件类型还原成线上字符串。
    ///
    /// `Other` 直接返回原始名称，保证迁移期间旧事件类型不丢。
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::TurnStarted => "turn_started",
            Self::TurnCompleted => "turn_completed",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::AssistantMessage => "assistant_message",
            Self::InteractionState => "interaction_state",
            Self::InteractionInterrupted => "interaction_interrupted",
            Self::Checkpoint => "checkpoint",
            Self::SessionRecovered => "session_recovered",
            Self::TurnMetrics => "turn_metrics",
            Self::JobStarted => "job_started",
            Self::JobCompleted => "job_completed",
            Self::Other(name) => name,
        }
    }

    /// 从线上字符串构造事件类型。
    #[must_use]
    pub fn from_str_lossy(event_type: &str) -> Self {
        match event_type {
            "turn_started" => Self::TurnStarted,
            "turn_completed" => Self::TurnCompleted,
            "tool_call" => Self::ToolCall,
            "tool_result" => Self::ToolResult,
            "assistant_message" => Self::AssistantMessage,
            "interaction_state" => Self::InteractionState,
            "interaction_interrupted" => Self::InteractionInterrupted,
            "checkpoint" => Self::Checkpoint,
            "session_recovered" => Self::SessionRecovered,
            "turn_metrics" => Self::TurnMetrics,
            "job_started" => Self::JobStarted,
            "job_completed" => Self::JobCompleted,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// 一条 durable 事件记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEventRecord {
    /// 会话内单调递增的序号。
    pub sequence: u64,
    /// 产生时刻（Unix 毫秒）。
    pub emitted_at_ms: u64,
    /// 写入时的 Host 代际，用于事后仲裁过期代际的帧。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    /// 归属的入站消息；会话级事件为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<String>,
    /// 来源渠道；仅 `External` 有值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_channel: Option<String>,
    /// 来源类别。
    #[serde(default)]
    pub origin_kind: TurnOrigin,
    /// 事件类型。
    pub event_type: RuntimeEventKind,
    /// 事件载荷。
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl RuntimeEventRecord {
    /// 构造一条事件。
    #[must_use]
    pub fn new(
        sequence: u64,
        emitted_at_ms: u64,
        event_type: RuntimeEventKind,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            sequence,
            emitted_at_ms,
            generation: None,
            interaction_id: None,
            origin_channel: None,
            origin_kind: TurnOrigin::External,
            event_type,
            payload,
        }
    }

    /// 标注归属的入站消息。
    #[must_use]
    pub fn with_interaction(mut self, interaction_id: impl Into<String>) -> Self {
        self.interaction_id = Some(interaction_id.into());
        self
    }

    /// 标注来源渠道与类别。
    #[must_use]
    pub fn with_origin(mut self, channel: Option<impl Into<String>>, kind: TurnOrigin) -> Self {
        self.origin_channel = channel.map(Into::into);
        self.origin_kind = kind;
        self
    }

    /// 标注写入时的 Host 代际。
    #[must_use]
    pub fn with_generation(mut self, generation: impl Into<String>) -> Self {
        self.generation = Some(generation.into());
        self
    }

    /// 构造一条单轮指标事件。
    pub fn turn_metrics(
        sequence: u64,
        emitted_at_ms: u64,
        metrics: &TurnMetrics,
    ) -> Result<Self, EventLogError> {
        let payload =
            serde_json::to_value(metrics).map_err(|error| EventLogError::Codec(error.to_string()))?;
        Ok(Self::new(
            sequence,
            emitted_at_ms,
            RuntimeEventKind::TurnMetrics,
            payload,
        ))
    }

    /// 把本条记录编码成一行 JSON（不含换行）。
    pub fn encode_line(&self) -> Result<Vec<u8>, EventLogError> {
        serde_json::to_vec(self).map_err(|error| EventLogError::Codec(error.to_string()))
    }

    /// 从一行 JSON 解码。
    pub fn decode_line(line: &[u8]) -> Result<Self, EventLogError> {
        serde_json::from_slice(line).map_err(|error| EventLogError::Codec(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips_through_json() {
        let mut metrics = TurnMetrics {
            turn_id: "turn-1".to_owned(),
            ttft_ms: Some(320),
            active_ms: 1_400,
            provider_duration_ms: 1_100,
            tool_ms: 200,
            input_tokens: 1_000,
            output_tokens: 240,
            ..TurnMetrics::default()
        };
        metrics.generation_speed_tps = Some(41.5);

        let record = RuntimeEventRecord::turn_metrics(7, 1_700_000_000_000, &metrics)
            .expect("指标事件应可构造")
            .with_generation("gen-1")
            .with_interaction("interaction-1")
            .with_origin(Some("desktop"), TurnOrigin::External);

        let line = record.encode_line().expect("可编码");
        assert!(!line.contains(&b'\n'), "单条记录不得包含换行");
        let decoded = RuntimeEventRecord::decode_line(&line).expect("可解码");
        assert_eq!(decoded, record);
    }

    #[test]
    fn unmeasured_ttft_is_absent_not_zero() {
        let record = RuntimeEventRecord::new(
            0,
            1,
            RuntimeEventKind::TurnMetrics,
            serde_json::json!({"turnId": "t"}),
        );
        let line = record.encode_line().expect("可编码");
        let text = String::from_utf8(line).expect("UTF-8");
        assert!(!text.contains("ttftMs"), "未测得首字不得出现在载荷里：{text}");
    }

    #[test]
    fn unknown_event_types_round_trip_by_name() {
        let record = RuntimeEventRecord::new(
            0,
            1,
            RuntimeEventKind::from_str_lossy("wire_event"),
            serde_json::json!({"k": 1}),
        );
        assert_eq!(record.event_type.as_str(), "wire_event");
        let line = record.encode_line().expect("可编码");
        let decoded = RuntimeEventRecord::decode_line(&line).expect("可解码");
        assert_eq!(decoded.event_type, RuntimeEventKind::Other("wire_event".to_owned()));
    }
}
