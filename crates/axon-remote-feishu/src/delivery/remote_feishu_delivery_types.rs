//! Feishu 远控 delivery 公共类型。

use axon_domain::SessionId;

use crate::{RemoteNotificationAdapterSendReport, RemoteNotificationAdapterSendStatus};

/// 默认 Feishu OpenAPI 基址。
pub const REMOTE_FEISHU_API_BASE_URL: &str = "https://open.feishu.cn";
/// 获取 tenant access token 相对路径。
pub const REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH: &str =
    "/open-apis/auth/v3/tenant_access_token/internal";
/// 发送消息相对路径。
pub const REMOTE_FEISHU_MESSAGE_CREATE_PATH: &str = "/open-apis/im/v1/messages";
/// 回复消息路径后缀。
pub const REMOTE_FEISHU_MESSAGE_REPLY_PATH_SUFFIX: &str = "/reply";
/// 请求计划里的 app_secret 占位符。
pub const REMOTE_FEISHU_APP_SECRET_PLACEHOLDER: &str = "__APP_SECRET__";
/// Feishu delivery 默认发送重试次数。
pub const REMOTE_FEISHU_DELIVERY_SEND_ATTEMPTS: u8 = 3;
/// reply 失败后允许 fallback create 的 Feishu 业务错误码。
pub const REMOTE_FEISHU_REPLY_FALLBACK_CODES: [i64; 2] = [230_011, 231_003];
/// delivery 请求已规划原因。
pub const REMOTE_FEISHU_DELIVERY_REASON_REQUEST_PLANNED: &str =
    "remote_feishu_delivery:request_planned";
/// delivery feedback 已规划原因。
pub const REMOTE_FEISHU_DELIVERY_REASON_FEEDBACK_PLANNED: &str =
    "remote_feishu_delivery:feedback_planned";
/// delivery 执行成功原因。
pub const REMOTE_FEISHU_DELIVERY_REASON_EXECUTED: &str = "remote_feishu_delivery:executed";
/// delivery 执行失败原因。
pub const REMOTE_FEISHU_DELIVERY_REASON_EXECUTION_FAILED: &str =
    "remote_feishu_delivery:execution_failed";

/// Feishu HTTP 请求计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuHttpRequestPlan {
    /// dispatch id。
    pub dispatch_id: String,
    /// 请求 URL。
    pub url: String,
    /// HTTP 方法。
    pub method: String,
    /// 请求头。
    pub headers: Vec<(String, String)>,
    /// JSON 请求体。
    pub body_json: String,
}

/// Feishu 消息目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMessageTarget {
    /// create 路径下的 receive_id。
    pub receive_id: String,
    /// create 路径下的 receive_id_type。
    pub receive_id_type: String,
    /// 可选 reply 目标消息 ID。
    pub reply_to_message_id: Option<String>,
}

/// Feishu delivery 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuDeliveryPlan {
    /// 目标会话。
    pub session_id: SessionId,
    /// token 请求。
    pub token_request: RemoteFeishuHttpRequestPlan,
    /// 消息发送请求。
    pub message_request: RemoteFeishuHttpRequestPlan,
    /// reply 失败时的 create fallback 请求。
    pub fallback_message_request: Option<RemoteFeishuHttpRequestPlan>,
    /// 是否允许真实发送。
    pub network_send_enabled: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Feishu HTTP 响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuHttpResponse {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应体。
    pub body: String,
}

/// Feishu delivery 执行报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuDeliveryExecutionReport {
    /// 目标会话。
    pub session_id: SessionId,
    /// dispatch id。
    pub dispatch_id: String,
    /// 是否尝试了真实网络发送。
    pub network_send_attempted: bool,
    /// tenant_access_token 是否获取成功。
    pub token_obtained: bool,
    /// Feishu message_id。
    pub adapter_message_id: Option<String>,
    /// 最终发送状态。
    pub send_status: RemoteNotificationAdapterSendStatus,
    /// 回写计划。
    pub feedback: RemoteNotificationAdapterSendReport,
    /// 可审计原因。
    pub reasons: Vec<String>,
}
