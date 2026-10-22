//! 统一 Gateway API token 鉴权；不绑定 HTTP 或 WebSocket 具体传输。

use std::env;

const BEARER_PREFIX: &str = "Bearer ";
/// URL 查询串里承载 API token 的参数名。
///
/// 部分发送方（Jira Server WebHooks 等）无法配置 HTTP header，只能把凭据写在 URL 上。
pub const GATEWAY_QUERY_TOKEN_PARAM: &str = "api_token";

/// Gateway token 加载错误。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GatewayAuthError {
    /// 配置的环境变量名为空或非法。
    #[error("Gateway API token 环境变量名无效")]
    InvalidEnvironmentName,
    /// 环境变量未配置有效 token。
    #[error("Gateway API token 未配置")]
    MissingToken,
}

/// 启动时加载一次的 Gateway API token。
#[derive(Clone)]
pub struct GatewayAuth {
    token: Box<str>,
}

impl GatewayAuth {
    /// 从指定环境变量读取 token；调用方应在 daemon 已加载相邻 `.env` 后调用。
    pub fn from_env_name(env_name: &str) -> Result<Self, GatewayAuthError> {
        let env_name = env_name.trim();
        if env_name.is_empty()
            || !env_name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return Err(GatewayAuthError::InvalidEnvironmentName);
        }
        let token = env::var(env_name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or(GatewayAuthError::MissingToken)?;
        Self::from_token(&token)
    }

    /// 从既有凭据存储读取出的 secret 值构造鉴权器。
    pub fn from_token(token: &str) -> Result<Self, GatewayAuthError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(GatewayAuthError::MissingToken);
        }
        Ok(Self {
            token: token.into(),
        })
    }

    /// 校验完整 Authorization header；不会记录或返回 token 原文。
    #[must_use]
    pub fn authorize_header(&self, header: Option<&str>) -> bool {
        let Some(value) = header.and_then(|value| value.strip_prefix(BEARER_PREFIX)) else {
            return false;
        };
        constant_time_equal(value.as_bytes(), self.token.as_bytes())
    }

    /// 供 transport 在创建连接资源前执行鉴权。
    #[must_use]
    pub fn authorize_bearer(&self, token: &str) -> bool {
        constant_time_equal(token.as_bytes(), self.token.as_bytes())
    }

    /// 校验入站请求凭据：`Authorization: Bearer` 或 URL 查询串 token，任一命中即通过。
    ///
    /// 查询串通道面向无法配置 HTTP header 的发送方；两条通道各自独立校验，
    /// 都失败才拒绝，因此不存在一条通道削弱另一条的情况。
    #[must_use]
    pub fn authorize_request(&self, header: Option<&str>, query_token: Option<&str>) -> bool {
        self.authorize_header(header)
            || query_token.is_some_and(|token| self.authorize_bearer(token))
    }
}

/// 从请求目标（路径 + 查询串）中取出 token 参数值。
///
/// 不做百分号解码：token 必须由 URL 安全字符构成，否则一律视为未提供（fail-closed）。
#[must_use]
pub fn query_token(path: &str) -> Option<&str> {
    path.split_once('?')?.1.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == GATEWAY_QUERY_TOKEN_PARAM && !value.is_empty()).then_some(value)
    })
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for index in 0..max_len {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(left_byte ^ right_byte);
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_header_requires_exact_constant_time_token_match() {
        let auth = GatewayAuth {
            token: "secret-token".into(),
        };
        assert!(auth.authorize_header(Some("Bearer secret-token")));
        assert!(!auth.authorize_header(Some("Bearer secret-token-extra")));
        assert!(!auth.authorize_header(Some("Basic secret-token")));
        assert!(!auth.authorize_header(None));
    }

    #[test]
    fn request_authorization_accepts_header_or_query_token() {
        let auth = GatewayAuth::from_token("secret-token").expect("token");
        assert!(auth.authorize_request(Some("Bearer secret-token"), None));
        assert!(auth.authorize_request(None, Some("secret-token")));
        assert!(!auth.authorize_request(Some("Bearer wrong"), None));
        assert!(!auth.authorize_request(None, Some("wrong")));
        assert!(!auth.authorize_request(None, None));
    }

    #[test]
    fn query_token_is_extracted_without_url_decoding() {
        assert_eq!(
            query_token("/gateway/v1/events/jira/raw?api_token=abc"),
            Some("abc")
        );
        assert_eq!(
            query_token("/gateway/v1/events?source=jira&api_token=abc&x=1"),
            Some("abc")
        );
        assert_eq!(query_token("/gateway/v1/events/jira/raw"), None);
        assert_eq!(query_token("/gateway/v1/events/jira/raw?api_token="), None);
        assert_eq!(query_token("/gateway/v1/events/jira/raw?other=abc"), None);
        assert_eq!(
            query_token("/gateway/v1/events/jira/raw?api_token=a&api_token=b"),
            Some("a")
        );
    }
}
