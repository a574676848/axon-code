//! 主代理运行身份领域合同。

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::{AxonError, AxonResult};

/// 本机所有者标识的固定前缀。
pub const AXON_OWNER_ID_PREFIX: &str = "owner_";
/// UUID 文本长度。
const UUID_TEXT_LENGTH: usize = 36;

/// 由可信本机运行时创建的稳定所有者标识。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AxonOwnerId(String);

impl AxonOwnerId {
    /// 从持久化文本恢复并校验所有者标识。
    pub fn parse(value: impl Into<String>) -> AxonResult<Self> {
        let value = value.into();
        if !valid_owner_id(&value) {
            return Err(AxonError::memory_persistence("Axon owner ID 格式无效"));
        }
        Ok(Self(value))
    }

    /// 返回稳定文本表示。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for AxonOwnerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AxonOwnerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(|_| D::Error::custom("Axon owner ID 格式无效"))
    }
}

/// 一次主代理运行绑定的可信 principal。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPrincipalContext {
    /// 本机所有者标识。
    pub owner_id: AxonOwnerId,
    /// 已认证渠道主体；本机 CLI 可为空。
    pub actor_ref: Option<String>,
    /// 运行入口引用。
    pub channel_ref: String,
}

fn valid_owner_id(value: &str) -> bool {
    let Some(uuid) = value.strip_prefix(AXON_OWNER_ID_PREFIX) else {
        return false;
    };
    uuid.len() == UUID_TEXT_LENGTH
        && uuid.chars().enumerate().all(|(index, character)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                character == '-'
            } else {
                character.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_id_requires_prefixed_uuid() {
        let owner = AxonOwnerId::parse("owner_550e8400-e29b-41d4-a716-446655440000")
            .expect("合法 owner ID 应通过校验");
        assert_eq!(owner.as_str(), "owner_550e8400-e29b-41d4-a716-446655440000");
        assert!(AxonOwnerId::parse("current-user").is_err());
    }
}
