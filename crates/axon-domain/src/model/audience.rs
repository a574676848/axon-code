use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::fmt;

/// 渠道标识和目标字段的最大长度。
pub const MAX_ROUTE_FIELD_LENGTH: usize = 256;

/// 受众类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudienceKind {
    /// 一对一交流。
    Private,
    /// 多人交流。
    Group,
}

/// 可披露内容的边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosurePolicy {
    /// 仅允许披露当前私密上下文。
    PrivateOnly,
    /// 允许披露当前 Workspace 范围内容。
    Workspace,
    /// 允许披露全局范围内容。
    Global,
}

/// 回复投递路由，不携带具体渠道 SDK 类型。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ReplyRoute {
    channel: String,
    destination: String,
}

impl<'de> Deserialize<'de> for ReplyRoute {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ReplyRouteData {
            channel: String,
            destination: String,
        }

        let data = ReplyRouteData::deserialize(deserializer)?;
        Self::new(data.channel, data.destination).map_err(D::Error::custom)
    }
}

impl ReplyRoute {
    /// 创建回复路由。
    pub fn new(
        channel: impl Into<String>,
        destination: impl Into<String>,
    ) -> Result<Self, AudienceError> {
        let channel = validate_route_field(channel.into(), "channel")?;
        let destination = validate_route_field(destination.into(), "destination")?;
        Ok(Self {
            channel,
            destination,
        })
    }

    /// 返回渠道标识。
    #[must_use]
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// 返回投递目标。
    #[must_use]
    pub fn destination(&self) -> &str {
        &self.destination
    }
}

/// 当前交互的受众与披露边界。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudienceContext {
    kind: AudienceKind,
    disclosure: DisclosurePolicy,
}

impl AudienceContext {
    /// 创建受众上下文。
    #[must_use]
    pub const fn new(kind: AudienceKind, disclosure: DisclosurePolicy) -> Self {
        Self { kind, disclosure }
    }

    /// 返回受众类型。
    #[must_use]
    pub const fn kind(&self) -> AudienceKind {
        self.kind
    }

    /// 返回披露策略。
    #[must_use]
    pub const fn disclosure(&self) -> DisclosurePolicy {
        self.disclosure
    }
}

/// 受众相关校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudienceError {
    /// 路由字段为空。
    EmptyRouteField(&'static str),
    /// 路由字段过长。
    RouteFieldTooLong(&'static str),
}

impl fmt::Display for AudienceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRouteField(field) => write!(formatter, "{field} 不能为空"),
            Self::RouteFieldTooLong(field) => {
                write!(
                    formatter,
                    "{field} 不能超过 {MAX_ROUTE_FIELD_LENGTH} 个字符"
                )
            }
        }
    }
}

impl std::error::Error for AudienceError {}

fn validate_route_field(value: String, field: &'static str) -> Result<String, AudienceError> {
    if value.is_empty() {
        return Err(AudienceError::EmptyRouteField(field));
    }
    if value.chars().count() > MAX_ROUTE_FIELD_LENGTH {
        return Err(AudienceError::RouteFieldTooLong(field));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_route_round_trips_and_rejects_invalid_fields() {
        let route = ReplyRoute::new("cli", "terminal-1").expect("合法路由");
        let value = serde_json::to_value(&route).expect("路由应可序列化");
        let decoded: ReplyRoute = serde_json::from_value(value).expect("路由应可反序列化");
        assert_eq!(decoded, route);
        assert!(ReplyRoute::new("", "terminal-1").is_err());
        assert!(ReplyRoute::new("cli", "").is_err());
        assert!(ReplyRoute::new("cli", "x".repeat(MAX_ROUTE_FIELD_LENGTH + 1)).is_err());
    }

    #[test]
    fn reply_route_deserialization_rejects_invalid_fields() {
        for value in [
            serde_json::json!({"channel": "", "destination": "terminal"}),
            serde_json::json!({"channel": "cli", "destination": ""}),
        ] {
            assert!(serde_json::from_value::<ReplyRoute>(value).is_err());
        }
    }
}
