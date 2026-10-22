//! 引用消息的 transport-neutral 领域合同。

use serde::{Deserialize, Serialize};

use crate::{AxonError, AxonResult};

/// 用户输入显式引用的消息上下文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotedMessageContext {
    /// 被引用消息的稳定 ID。
    pub message_id: String,
    /// 被引用消息的 Agent 可见正文。
    pub visible_text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuotedMessageContextDto {
    message_id: String,
    visible_text: String,
}

impl<'de> Deserialize<'de> for QuotedMessageContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = QuotedMessageContextDto::deserialize(deserializer)?;
        Self::new(value.message_id, value.visible_text)
            .map_err(|_| serde::de::Error::custom("引用消息 ID 与可见正文不能为空"))
    }
}

impl QuotedMessageContext {
    /// 创建规范化引用上下文。
    pub fn new(message_id: impl Into<String>, visible_text: impl Into<String>) -> AxonResult<Self> {
        let message_id = message_id.into().trim().to_owned();
        let visible_text = visible_text.into().trim().to_owned();
        if message_id.is_empty() || visible_text.is_empty() {
            return Err(AxonError::runtime_failure("引用消息 ID 与可见正文不能为空"));
        }
        Ok(Self {
            message_id,
            visible_text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_message_context_normalizes_non_empty_fields() {
        let context = QuotedMessageContext::new(" om_parent ", " 父消息 ").unwrap();

        assert_eq!(context.message_id, "om_parent");
        assert_eq!(context.visible_text, "父消息");
    }

    #[test]
    fn quoted_message_context_rejects_empty_fields() {
        assert!(QuotedMessageContext::new("", "父消息").is_err());
        assert!(QuotedMessageContext::new("om_parent", "  ").is_err());
    }

    #[test]
    fn quoted_message_context_deserialization_enforces_domain_invariants() {
        let error = serde_json::from_str::<QuotedMessageContext>(
            r#"{"messageId":"om_parent","visibleText":"   "}"#,
        )
        .expect_err("空正文必须被拒绝");

        assert!(error.to_string().contains("不能为空"));
    }
}
