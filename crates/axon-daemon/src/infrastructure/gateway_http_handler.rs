//! HTTP Gateway 外部路由扩展处理器。

use axon_protocol::SubmitInteractionReceipt;

/// 自定义 HTTP 响应模型。
#[derive(Debug, Clone)]
pub struct GatewayHttpResponse {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应 Content-Type 标头。
    pub content_type: String,
    /// 响应体字节流。
    pub body: Vec<u8>,
}

impl GatewayHttpResponse {
    /// 构造 JSON 响应。
    #[must_use]
    pub fn json(status: u16, value: &impl serde::Serialize) -> Self {
        Self {
            status,
            content_type: "application/json".to_owned(),
            body: serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
        }
    }

    /// 构造 CSV 响应。
    #[must_use]
    pub fn csv(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: "text/csv; charset=utf-8".to_owned(),
            body,
        }
    }

    /// 构造统一错误模型响应。
    #[must_use]
    pub fn error(
        status: u16,
        code: &str,
        message: &str,
        details: Option<serde_json::Value>,
    ) -> Self {
        let payload = serde_json::json!({
            "error": {
                "code": code,
                "message": message,
                "details": details,
            }
        });
        Self::json(status, &payload)
    }

    /// 构造事件接收回执响应。
    #[must_use]
    pub fn receipt(receipt: &SubmitInteractionReceipt) -> Self {
        Self::json(202, receipt)
    }
}

/// 外部 HTTP 路由处理器接口。
pub trait GatewayHttpRouteHandler: Send + Sync {
    /// 尝试处理外部请求。未命中对应路由时返回 None。
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse>;
}
