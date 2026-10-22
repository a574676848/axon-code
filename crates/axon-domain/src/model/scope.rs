use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::fmt;

/// Workspace 标识的最大长度。
pub const MAX_WORKSPACE_ID_LENGTH: usize = 128;

/// 作用域校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeError {
    /// Workspace 标识为空。
    EmptyWorkspaceId,
    /// Workspace 标识超过长度上限。
    WorkspaceIdTooLong,
    /// Workspace 标识包含不允许的字符。
    InvalidWorkspaceIdCharacter(char),
}

impl fmt::Display for ScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyWorkspaceId => formatter.write_str("Workspace ID 不能为空"),
            Self::WorkspaceIdTooLong => {
                write!(
                    formatter,
                    "Workspace ID 不能超过 {MAX_WORKSPACE_ID_LENGTH} 个字符"
                )
            }
            Self::InvalidWorkspaceIdCharacter(character) => {
                write!(formatter, "Workspace ID 包含非法字符：{character}")
            }
        }
    }
}

impl std::error::Error for ScopeError {}

/// 经过校验的 Workspace 标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// 创建并校验 Workspace 标识。
    pub fn new(value: impl Into<String>) -> Result<Self, ScopeError> {
        let value = value.into();
        validate_workspace_id(&value)?;
        Ok(Self(value))
    }

    /// 返回稳定的字符串视图。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for WorkspaceId {
    type Error = ScopeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for WorkspaceId {
    type Error = ScopeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for WorkspaceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkspaceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// JARVIS 的全局或 Workspace 作用域。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "workspace_id", rename_all = "snake_case")]
pub enum ScopeRef {
    /// 全局作用域。
    Global,
    /// 指定 Workspace 作用域。
    Workspace(WorkspaceId),
}

impl ScopeRef {
    /// 创建全局作用域。
    #[must_use]
    pub const fn global() -> Self {
        Self::Global
    }

    /// 创建 Workspace 作用域并校验标识。
    pub fn workspace(value: impl Into<String>) -> Result<Self, ScopeError> {
        WorkspaceId::new(value).map(Self::Workspace)
    }

    /// 返回 Workspace 标识；全局作用域返回 None。
    #[must_use]
    pub fn workspace_id(&self) -> Option<&WorkspaceId> {
        match self {
            Self::Global => None,
            Self::Workspace(workspace_id) => Some(workspace_id),
        }
    }
}

fn validate_workspace_id(value: &str) -> Result<(), ScopeError> {
    if value.is_empty() {
        return Err(ScopeError::EmptyWorkspaceId);
    }
    if value.chars().count() > MAX_WORKSPACE_ID_LENGTH {
        return Err(ScopeError::WorkspaceIdTooLong);
    }
    value
        .chars()
        .find(|character| {
            !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_' | '.')
        })
        .map_or(Ok(()), |character| {
            Err(ScopeError::InvalidWorkspaceIdCharacter(character))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_scope_serializes_without_workspace_id() {
        let value = serde_json::to_value(ScopeRef::global()).expect("全局作用域应可序列化");
        assert_eq!(value, serde_json::json!({"kind": "global"}));
    }

    #[test]
    fn workspace_scope_round_trips() {
        let scope = ScopeRef::workspace("workspace-a").expect("合法 Workspace ID");
        let encoded = serde_json::to_value(&scope).expect("Workspace 作用域应可序列化");
        assert_eq!(
            encoded,
            serde_json::json!({"kind": "workspace", "workspace_id": "workspace-a"})
        );
        let decoded: ScopeRef =
            serde_json::from_value(encoded).expect("Workspace 作用域应可反序列化");
        assert_eq!(decoded, scope);
    }

    #[test]
    fn invalid_workspace_ids_are_rejected() {
        for value in ["", "workspace/id", "workspace id", "工作区"] {
            assert!(
                ScopeRef::workspace(value).is_err(),
                "应拒绝非法 ID：{value}"
            );
        }
        let oversized = "x".repeat(MAX_WORKSPACE_ID_LENGTH + 1);
        assert!(ScopeRef::workspace(oversized).is_err());
        let malformed = serde_json::json!({"kind": "workspace", "workspace_id": "workspace/id"});
        assert!(serde_json::from_value::<ScopeRef>(malformed).is_err());
    }
}
