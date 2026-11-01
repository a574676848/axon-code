//! 统一 Gateway 鉴权：全局 token 与可热更新的多 Key，返回身份而不是布尔值。
//!
//! 两条凭据通道：
//!
//! - 全局 token（`remote.gateway.api_token_env` 指向的那把）：身份是 [`GatewayIdentity::SuperAdmin`]，
//!   可访问全部业务路由与管理面；
//! - 由超管签发的 Key：身份是 [`GatewayIdentity::Key`]，可访问全部业务路由，管理面一律 403。
//!
//! 判定结果必须是身份而不是布尔值：路由层据此分级（`/api/v1/keys` 只接受超管）。多 Key
//! 存储在内存里，`create`/`revoke` 之后下一次鉴权立即按新状态判定，不要求重启 daemon。

#[path = "gateway_keys.rs"]
mod gateway_keys;

use std::env;
use std::sync::Arc;

pub use gateway_keys::{
    DEFAULT_KEY_STORE_RELATIVE_PATH, GatewayKeyError, GatewayKeyStore, IssuedKey, KEY_HASH_ALGO,
    KEY_HASH_ITERATIONS, KeyIdentity, KeyOrigin, KeySummary,
};

const BEARER_PREFIX: &str = "Bearer ";
/// URL 查询串里承载 API token 的参数名。
///
/// 部分发送方（Jira Server WebHooks 等）无法配置 HTTP header，只能把凭据写在 URL 上。
pub const GATEWAY_QUERY_TOKEN_PARAM: &str = "api_token";

/// 请求身份；由鉴权层返回，路由层据此分级。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayIdentity {
    /// 全局 token，即超管。
    SuperAdmin,
    /// 由超管签发的 Key。
    Key {
        /// Key 标识（不是秘密）。
        id: String,
        /// Key 标签（不是秘密）。
        label: String,
    },
}

impl GatewayIdentity {
    /// 是否为超管。
    #[must_use]
    pub fn is_super_admin(&self) -> bool {
        matches!(self, Self::SuperAdmin)
    }

    /// 身份描述，供日志与审计使用；不含明文与摘要。
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::SuperAdmin => "superadmin".to_owned(),
            Self::Key { id, .. } => format!("key:{id}"),
        }
    }
}

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

/// 可热更新的 Gateway 鉴权器。
///
/// 既有构造入口（[`Self::from_env_name`] 与 [`Self::from_token`]）的语义不变：它们拿到的是
/// 只启用全局 token 的鉴权器，行为与改造前逐字一致。多 Key 由调用方用 [`Self::with_key_store`]
/// 挂上同一份 [`GatewayKeyStore`]，旧 token 通道的退役开关由 [`Self::with_legacy_token_enabled`]
/// 控制。
#[derive(Clone)]
pub struct GatewayAuth {
    /// 全局 token；只用于常数时间比较，不参与任何格式化输出。
    token: Box<str>,
    /// 多 Key 存储；未配置时该通道整体不启用。
    keys: Option<Arc<GatewayKeyStore>>,
    /// 旧 token 通道是否启用；关闭后全局 token 不再对业务路由生效。
    legacy_token_enabled: bool,
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
            keys: None,
            legacy_token_enabled: true,
        })
    }

    /// 挂上多 Key 存储；调用方应与签发侧共享同一个 `Arc`，否则看不到彼此的改动。
    #[must_use]
    pub fn with_key_store(mut self, store: Arc<GatewayKeyStore>) -> Self {
        self.keys = Some(store);
        self
    }

    /// 设置旧 token 通道是否启用。
    ///
    /// 默认 `true`。置为 `false` 时全局 token 不再对业务路由生效，只有 Key 能用；管理面本身
    /// 只接受超管，因此关闭旧通道等于同时关闭管理面的可达性，这是该开关的既定后果。
    #[must_use]
    pub fn with_legacy_token_enabled(mut self, enabled: bool) -> Self {
        self.legacy_token_enabled = enabled;
        self
    }

    /// 多 Key 存储句柄；未配置时为 `None`。
    #[must_use]
    pub fn key_store(&self) -> Option<Arc<GatewayKeyStore>> {
        self.keys.clone()
    }

    /// 旧 token 通道是否启用。
    #[must_use]
    pub fn legacy_token_enabled(&self) -> bool {
        self.legacy_token_enabled
    }

    /// 校验完整 Authorization header；不会记录或返回 token 原文。
    #[must_use]
    pub fn authorize_header(&self, header: Option<&str>) -> bool {
        self.identify_header(header).is_some()
    }

    /// 供 transport 在创建连接资源前执行鉴权。
    #[must_use]
    pub fn authorize_bearer(&self, token: &str) -> bool {
        self.identify_bearer(token).is_some()
    }

    /// 校验入站请求凭据：`Authorization: Bearer` 或 URL 查询串 token，任一命中即通过。
    ///
    /// 查询串通道面向无法配置 HTTP header 的发送方；两条通道各自独立校验，
    /// 都失败才拒绝，因此不存在一条通道削弱另一条的情况。
    #[must_use]
    pub fn authorize_request(&self, header: Option<&str>, query_token: Option<&str>) -> bool {
        self.identify_request(header, query_token).is_some()
    }

    /// 按 Authorization header 解析身份；未命中返回 `None`。
    #[must_use]
    pub fn identify_header(&self, header: Option<&str>) -> Option<GatewayIdentity> {
        let value = header?.strip_prefix(BEARER_PREFIX)?;
        self.identify_bearer(value)
    }

    /// 按纯凭据解析身份。
    ///
    /// 先比全局 token（廉价常数时间比较），再查多 Key（一次 PBKDF2）：顺序固定，避免每个
    /// 请求都为了一把不存在的 Key 付派生成本。
    #[must_use]
    pub fn identify_bearer(&self, token: &str) -> Option<GatewayIdentity> {
        let token = token.trim();
        if token.is_empty() {
            return None;
        }
        if self.legacy_token_enabled && constant_time_equal(token.as_bytes(), self.token.as_bytes())
        {
            return Some(GatewayIdentity::SuperAdmin);
        }
        let keys = self.keys.as_deref()?;
        keys.verify(token).map(|identity| GatewayIdentity::Key {
            id: identity.id,
            label: identity.label,
        })
    }

    /// 按 header 与查询串 token 解析身份；header 优先，两条通道任一命中即给出身份。
    #[must_use]
    pub fn identify_request(
        &self,
        header: Option<&str>,
        query_token: Option<&str>,
    ) -> Option<GatewayIdentity> {
        self.identify_header(header).or_else(|| {
            query_token
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .and_then(|token| self.identify_bearer(token))
        })
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

/// 常数时间比较：长度差异与内容差异都不会提前返回。
///
/// 供多 Key 存储复用同一口径，避免出现两套强度不同的比较实现。
pub(crate) fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
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

    fn auth(token: &str) -> GatewayAuth {
        GatewayAuth::from_token(token).expect("token")
    }

    #[test]
    fn bearer_header_requires_exact_constant_time_token_match() {
        let auth = GatewayAuth {
            token: "secret-token".into(),
            keys: None,
            legacy_token_enabled: true,
        };
        assert!(auth.authorize_header(Some("Bearer secret-token")));
        assert!(!auth.authorize_header(Some("Bearer secret-token-extra")));
        assert!(!auth.authorize_header(Some("Basic secret-token")));
        assert!(!auth.authorize_header(None));
    }

    #[test]
    fn request_authorization_accepts_header_or_query_token() {
        let auth = auth("secret-token");
        assert!(auth.authorize_request(Some("Bearer secret-token"), None));
        assert!(auth.authorize_request(None, Some("secret-token")));
        assert!(!auth.authorize_request(Some("Bearer wrong"), None));
        assert!(!auth.authorize_request(None, Some("wrong")));
        assert!(!auth.authorize_request(None, None));
    }

    #[test]
    fn identity_grades_global_token_as_super_admin() {
        let auth = auth("secret-token");
        assert_eq!(
            auth.identify_request(Some("Bearer secret-token"), None),
            Some(GatewayIdentity::SuperAdmin)
        );
        assert_eq!(
            auth.identify_request(None, Some("secret-token")),
            Some(GatewayIdentity::SuperAdmin)
        );
        assert_eq!(auth.identify_request(None, None), None);
        assert!(GatewayIdentity::SuperAdmin.is_super_admin());
        assert_eq!(GatewayIdentity::SuperAdmin.describe(), "superadmin");
    }

    #[test]
    fn disabling_legacy_token_retires_the_global_token_channel() {
        let auth = auth("secret-token").with_legacy_token_enabled(false);
        assert!(!auth.authorize_header(Some("Bearer secret-token")));
        assert!(!auth.authorize_request(None, Some("secret-token")));
        assert_eq!(
            auth.identify_request(Some("Bearer secret-token"), None),
            None
        );
        assert!(!auth.legacy_token_enabled());
        assert!(auth.key_store().is_none(), "未挂存储时不应凭空造出一个");
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
