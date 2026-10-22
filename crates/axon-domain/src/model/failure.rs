//! 跨运行时、会话与协议层共享的结构化失败事实。

use serde::{Deserialize, Serialize};

/// 非持久化 synthetic failure 无可用发生时间时使用的哨兵值。
pub const RUNTIME_FAILURE_TIMESTAMP_UNKNOWN: u64 = 0;

/// Provider 协议失败类别。
pub const RUNTIME_FAILURE_CATEGORY_PROVIDER_PROTOCOL: &str = "provider_protocol";
/// 通用运行时失败类别。
pub const RUNTIME_FAILURE_CATEGORY_RUNTIME: &str = "runtime";
/// 用户中断类别。
pub const RUNTIME_FAILURE_CATEGORY_INTERRUPTED: &str = "interrupted";
/// Provider 工具调用不完整错误码。
pub const RUNTIME_FAILURE_CODE_PROVIDER_TOOL_CALL_INCOMPLETE: &str =
    "provider_tool_call_incomplete";
/// Provider 响应无效错误码。
pub const RUNTIME_FAILURE_CODE_PROVIDER_INVALID_RESPONSE: &str = "provider_invalid_response";
/// 通用运行时失败错误码。
pub const RUNTIME_FAILURE_CODE_RUNTIME_FAILED: &str = "runtime_failed";
/// 用户中断错误码。
pub const RUNTIME_FAILURE_CODE_INTERRUPTED: &str = "run_interrupted";
/// Provider incomplete 失败类别。
pub const RUNTIME_FAILURE_KIND_INCOMPLETE: &str = "incomplete";
/// Provider 响应无效失败类别。
pub const RUNTIME_FAILURE_KIND_INVALID_RESPONSE: &str = "invalid_response";

/// 可由 Session、wire 和各端投影共同消费的权威失败事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeFailure {
    /// 稳定错误类别。
    pub category: String,
    /// 稳定错误码。
    pub code: String,
    /// 发生 Provider 失败时使用的稳定协议编码。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_protocol: Option<String>,
    /// 已脱敏的可诊断消息。
    pub redacted_message: String,
    /// HTTP 状态码。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// 引发失败的运行时事件 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caused_by_event: Option<String>,
    /// 失败前是否已投影用户可见输出。
    pub visible_output_emitted: bool,
    /// 是否允许用户显式重试。
    pub retryable: bool,
    /// 是否允许运行时静默重放当前 turn。
    pub replay_safe: bool,
    /// 已执行的请求尝试次数。
    pub attempt_count: u32,
    /// Session 中是否保存了部分输出。
    pub partial_output_present: bool,
    /// 失败发生时间，Unix 毫秒。
    pub timestamp: u64,
    /// Provider/transport 失败类别。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    /// 不完整协议字段；不包含原始参数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_field: Option<String>,
    /// 不完整工具调用在响应中的索引。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_index: Option<usize>,
    /// 触发失败的 Provider 终态事件类型（如 `response.failed`）；非事件类失败为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_event_type: Option<String>,
    /// Provider 或边缘网关返回的请求 ID；上游未回传时为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_request_id: Option<String>,
}

impl RuntimeFailure {
    /// 创建结构化失败事实；调用方必须显式给出重放边界。
    #[must_use]
    pub fn new(
        category: impl Into<String>,
        code: impl Into<String>,
        redacted_message: impl Into<String>,
        timestamp: u64,
    ) -> Self {
        Self {
            category: category.into(),
            code: code.into(),
            provider_protocol: None,
            redacted_message: redacted_message.into(),
            http_status: None,
            caused_by_event: None,
            visible_output_emitted: false,
            retryable: false,
            replay_safe: false,
            attempt_count: 1,
            partial_output_present: false,
            timestamp,
            failure_kind: None,
            missing_field: None,
            call_index: None,
            provider_event_type: None,
            upstream_request_id: None,
        }
    }
}
