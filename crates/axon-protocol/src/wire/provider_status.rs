use serde::{Deserialize, Serialize};

/// Provider 请求采用的底层传输。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireProviderTransport {
    /// HTTP streaming。
    Http,
    /// WebSocket streaming。
    WebSocket,
}

/// Provider 请求在一次 Agent 运行中的实时阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireProviderStatusPhase {
    /// 正在建立请求连接。
    Connecting,
    /// 已收到 provider 的首个有效流事件。
    Connected,
    /// 已开始接收 reasoning 或回复内容。
    Generating,
    /// 当前 provider 正在执行传输重试。
    Retrying,
    /// 当前 provider 请求失败。
    Failed,
    /// 正在从当前 provider 切换到下一 provider。
    Failover,
}

/// Provider 请求实时状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireProviderStatus {
    /// 当前 provider 名称。
    pub provider: String,
    /// 当前模型 ID。
    pub model: String,
    /// 当前请求阶段。
    pub phase: WireProviderStatusPhase,
    /// 当前 provider 使用的底层传输。
    pub transport: WireProviderTransport,
    /// 当前 provider 内部传输尝试序号。
    pub attempt_index: u64,
    /// 失败响应的 HTTP 状态码。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// 已脱敏的失败摘要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// 故障转移目标 provider。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_provider: Option<String>,
    /// 故障转移目标模型 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_model: Option<String>,
}
