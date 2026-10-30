use super::{AudienceContext, ReplyRoute, ScopeRef};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::fmt;

/// Interaction 标识的最大长度。
pub const MAX_INTERACTION_ID_LENGTH: usize = 128;

/// Interaction 内容的最大字符数。
pub const MAX_INTERACTION_CONTENT_LENGTH: usize = 100_000;

/// 系统自身派发的 Interaction 渠道标识。
///
/// 该渠道的记录不参与「最近使用渠道」回落，避免完成通知投递目标自我循环。
pub const SYSTEM_SOURCE_CHANNEL: &str = "system";

/// 完成通知 steer Interaction 的来源 ID 前缀。
///
/// 这类 Interaction 由系统生成，只承载「把完成消息投递到某渠道」的指令，
/// 不属于用户真实渠道使用记录，因此「最近使用渠道」扫描必须排除它们。
pub const COMPLETION_STEER_SOURCE_PREFIX: &str = "completion-steer-";

/// Cron 唤醒回合的来源 ID 前缀。
///
/// 唤醒回合由系统派生，不属于用户真实输入：它必须独立成一轮，绝不能插入用户正在
/// 进行的回合。
pub const CRON_WAKE_SOURCE_PREFIX: &str = "cron-wake:";

/// Cron 投递项的稳定 ID 前缀。
///
/// 只用于任务投递项自身的身份派生，不参与 Interaction 来源判定：投递项与唤醒回合
/// 是两套独立动作，混用前缀会让同一轮结果被重复登记。
pub const CRON_DELIVERY_ID_PREFIX: &str = "cron:";

/// Cron 投递项通知 ID 前缀。
pub const CRON_DELIVERY_NOTIFICATION_PREFIX: &str = "cron-delivery-";

/// 交互记录标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct InteractionId(String);

impl<'de> Deserialize<'de> for InteractionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

impl InteractionId {
    /// 创建交互记录标识。
    pub fn new(value: impl Into<String>) -> Result<Self, InteractionError> {
        let value = value.into();
        if value.is_empty() {
            return Err(InteractionError::EmptyField("interaction_id"));
        }
        if value.chars().count() > MAX_INTERACTION_ID_LENGTH {
            return Err(InteractionError::FieldTooLong("interaction_id"));
        }
        Ok(Self(value))
    }

    /// 返回稳定的字符串视图。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 入口来源与幂等键。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct InteractionSource {
    channel: String,
    source_message_id: String,
}

impl<'de> Deserialize<'de> for InteractionSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct InteractionSourceData {
            channel: String,
            source_message_id: String,
        }

        let data = InteractionSourceData::deserialize(deserializer)?;
        Self::new(data.channel, data.source_message_id).map_err(D::Error::custom)
    }
}

impl InteractionSource {
    /// 创建来源信息。
    pub fn new(
        channel: impl Into<String>,
        source_message_id: impl Into<String>,
    ) -> Result<Self, InteractionError> {
        let channel = validate_field(channel.into(), "channel")?;
        let source_message_id = validate_field(source_message_id.into(), "source_message_id")?;
        Ok(Self {
            channel,
            source_message_id,
        })
    }

    /// 返回来源渠道。
    #[must_use]
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// 返回来源消息幂等键。
    #[must_use]
    pub fn source_message_id(&self) -> &str {
        &self.source_message_id
    }
}

/// Interaction 处理状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    /// 已可靠接收。
    Received,
    /// 已被主 turn 领取处理。
    Processing,
    /// 已完成理解并产生语义结果。
    Understood,
    /// 因安全或校验原因拒绝。
    Rejected,
    /// 进程中断后无法继续处理。
    Interrupted,
}

/// 已持久化的自然语言交互。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionRecord {
    /// 交互记录标识。
    pub interaction_id: InteractionId,
    /// 来源与幂等键。
    pub source: InteractionSource,
    /// 作用域。
    pub scope: ScopeRef,
    /// 受众与披露边界。
    pub audience: AudienceContext,
    /// 回复投递路由。
    pub reply_route: ReplyRoute,
    /// 原始交互内容。
    pub content: String,
    /// 渠道入站阶段下载并注入的本地附件绝对路径。
    ///
    /// 由渠道适配器在 durable append 之后、dispatch 之前填充，并投影为 Provider turn
    /// 的 `transportContext.files`：图片成为 vision 输入，文件成为可读附件。它只描述
    /// 输入素材，不携带任何调用方运行态身份字段。
    pub attachments: Vec<String>,
    /// 当前处理状态。
    pub status: InteractionStatus,
    /// 接收时间（毫秒）。
    pub received_at_ms: u64,
}

impl<'de> Deserialize<'de> for InteractionRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct InteractionRecordData {
            interaction_id: InteractionId,
            source: InteractionSource,
            scope: ScopeRef,
            audience: AudienceContext,
            reply_route: ReplyRoute,
            content: String,
            #[serde(default)]
            attachments: Vec<String>,
            status: InteractionStatus,
            received_at_ms: u64,
        }

        let data = InteractionRecordData::deserialize(deserializer)?;
        let mut record = Self::new(
            data.interaction_id,
            data.source,
            data.scope,
            data.audience,
            data.reply_route,
            data.content,
            data.received_at_ms,
        )
        .map_err(D::Error::custom)?;
        record.attachments = data.attachments;
        record.status = data.status;
        Ok(record)
    }
}

impl InteractionRecord {
    /// 创建待处理的 Interaction。
    pub fn new(
        interaction_id: InteractionId,
        source: InteractionSource,
        scope: ScopeRef,
        audience: AudienceContext,
        reply_route: ReplyRoute,
        content: impl Into<String>,
        received_at_ms: u64,
    ) -> Result<Self, InteractionError> {
        let content = content.into();
        if content.is_empty() {
            return Err(InteractionError::EmptyField("content"));
        }
        if content.chars().count() > MAX_INTERACTION_CONTENT_LENGTH {
            return Err(InteractionError::FieldTooLong("content"));
        }
        Ok(Self {
            interaction_id,
            source,
            scope,
            audience,
            reply_route,
            content,
            attachments: Vec::new(),
            status: InteractionStatus::Received,
            received_at_ms,
        })
    }
}

/// Interaction 数据校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionError {
    /// 字段为空。
    EmptyField(&'static str),
    /// 字段超过长度上限。
    FieldTooLong(&'static str),
}

impl fmt::Display for InteractionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField(field) => write!(formatter, "{field} 不能为空"),
            Self::FieldTooLong(field) => write!(formatter, "{field} 超过长度上限"),
        }
    }
}

impl std::error::Error for InteractionError {}

fn validate_field(value: String, field: &'static str) -> Result<String, InteractionError> {
    if value.is_empty() {
        return Err(InteractionError::EmptyField(field));
    }
    if value.chars().count() > MAX_INTERACTION_ID_LENGTH {
        return Err(InteractionError::FieldTooLong(field));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudienceKind, DisclosurePolicy};

    fn sample() -> InteractionRecord {
        InteractionRecord::new(
            InteractionId::new("interaction-1").expect("合法 ID"),
            InteractionSource::new("tui", "message-1").expect("合法来源"),
            ScopeRef::workspace("workspace-a").expect("合法作用域"),
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            ReplyRoute::new("tui", "terminal-1").expect("合法路由"),
            "请检查构建状态",
            42,
        )
        .expect("Interaction 应创建")
    }

    #[test]
    fn interaction_starts_received_and_round_trips() {
        let record = sample();
        assert_eq!(record.status, InteractionStatus::Received);
        let encoded = serde_json::to_value(&record).expect("Interaction 应可序列化");
        let decoded: InteractionRecord =
            serde_json::from_value(encoded).expect("Interaction 应可反序列化");
        assert_eq!(decoded, record);
    }

    #[test]
    fn interaction_rejects_empty_or_oversized_content() {
        let base = sample();
        assert!(
            InteractionRecord::new(
                base.interaction_id.clone(),
                base.source.clone(),
                base.scope.clone(),
                base.audience.clone(),
                base.reply_route.clone(),
                "",
                42,
            )
            .is_err()
        );
        assert!(
            InteractionRecord::new(
                base.interaction_id,
                base.source,
                base.scope,
                base.audience,
                base.reply_route,
                "x".repeat(MAX_INTERACTION_CONTENT_LENGTH + 1),
                42,
            )
            .is_err()
        );
    }

    #[test]
    fn interaction_deserialization_rejects_invalid_nested_fields_and_content() {
        let mut value = serde_json::to_value(sample()).expect("Interaction 应可序列化");

        value["interaction_id"] = serde_json::json!("");
        assert!(serde_json::from_value::<InteractionRecord>(value.clone()).is_err());

        value = serde_json::to_value(sample()).expect("Interaction 应可序列化");
        value["source"]["source_message_id"] = serde_json::json!("");
        assert!(serde_json::from_value::<InteractionRecord>(value.clone()).is_err());

        value = serde_json::to_value(sample()).expect("Interaction 应可序列化");
        value["content"] = serde_json::json!("");
        assert!(serde_json::from_value::<InteractionRecord>(value).is_err());
    }

    #[test]
    fn interaction_deserialization_rejects_runtime_identity_fields() {
        for field in ["sessionId", "threadId", "runId"] {
            let mut value = serde_json::to_value(sample()).expect("Interaction 应可序列化");
            value[field] = serde_json::json!("runtime-identity");

            let error = serde_json::from_value::<InteractionRecord>(value)
                .expect_err("运行时身份字段必须被拒绝，不得静默忽略");

            assert!(
                error.to_string().contains(field),
                "错误信息应指向未知字段 {field}：{error}"
            );
        }
    }
}
