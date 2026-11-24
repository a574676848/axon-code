//! 结构化 cell payload schema 与注册表。

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::payloads::{
    CommandCellPayload, DiffCellPayload, ErrorCellPayload, ExecCellPayload,
    GenerationStatusCellPayload, HistoryCellPayload, HookCellPayload, McpCellPayload,
    MessageCellPayload, PlanCellPayload, QuestionCellPayload, ReasoningCellPayload,
    ReviewCellPayload, StatusCellPayload, TurnSummaryCellPayload, UsageCellPayload,
};

/// Cell payload 稳定 kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellPayloadKind(&'static str);

/// 可落盘 cell payload。
pub trait CellPayload: Serialize + fmt::Debug + Send + Sync {
    /// 返回稳定 payload kind。
    fn kind(&self) -> CellPayloadKind;
}

/// 可序列化的 cell payload envelope。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CellPayloadEnvelope {
    /// 稳定 payload kind。
    pub kind: String,
    /// 具体 payload 内容。
    pub payload: Value,
}

/// Cell payload schema 错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellPayloadSchemaError {
    kind: String,
    message: String,
}

#[derive(Clone, Copy)]
struct CellPayloadRegistryEntry {
    kind: CellPayloadKind,
    validate: fn(CellPayloadKind, Value) -> Result<Value, CellPayloadSchemaError>,
}

const CELL_PAYLOAD_REGISTRY: &[CellPayloadRegistryEntry] = &[
    entry::<MessageCellPayload>(MessageCellPayload::KIND),
    entry::<ExecCellPayload>(ExecCellPayload::KIND),
    entry::<DiffCellPayload>(DiffCellPayload::KIND),
    entry::<PlanCellPayload>(PlanCellPayload::KIND),
    entry::<ReviewCellPayload>(ReviewCellPayload::KIND),
    entry::<HookCellPayload>(HookCellPayload::KIND),
    entry::<StatusCellPayload>(StatusCellPayload::KIND),
    entry::<GenerationStatusCellPayload>(GenerationStatusCellPayload::KIND),
    entry::<ReasoningCellPayload>(ReasoningCellPayload::KIND),
    entry::<TurnSummaryCellPayload>(TurnSummaryCellPayload::KIND),
    entry::<UsageCellPayload>(UsageCellPayload::KIND),
    entry::<ErrorCellPayload>(ErrorCellPayload::KIND),
    entry::<HistoryCellPayload>(HistoryCellPayload::KIND),
    entry::<CommandCellPayload>(CommandCellPayload::KIND),
    entry::<McpCellPayload>(McpCellPayload::KIND),
    entry::<QuestionCellPayload>(QuestionCellPayload::KIND),
];

const fn entry<T>(kind: CellPayloadKind) -> CellPayloadRegistryEntry
where
    T: CellPayload + DeserializeOwned,
{
    CellPayloadRegistryEntry {
        kind,
        validate: validate_payload::<T>,
    }
}

impl CellPayloadKind {
    /// 创建稳定 payload kind。
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    /// 返回字符串编码。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[cfg(test)]
const EXPECTED_CELL_PAYLOAD_KINDS: &[CellPayloadKind] = &[
    MessageCellPayload::KIND,
    ExecCellPayload::KIND,
    DiffCellPayload::KIND,
    PlanCellPayload::KIND,
    ReviewCellPayload::KIND,
    HookCellPayload::KIND,
    StatusCellPayload::KIND,
    GenerationStatusCellPayload::KIND,
    ReasoningCellPayload::KIND,
    TurnSummaryCellPayload::KIND,
    UsageCellPayload::KIND,
    ErrorCellPayload::KIND,
    HistoryCellPayload::KIND,
    CommandCellPayload::KIND,
    McpCellPayload::KIND,
    QuestionCellPayload::KIND,
];

impl CellPayloadEnvelope {
    /// 从强类型 payload 创建 envelope。
    pub fn from_payload<P>(payload: &P) -> Result<Self, CellPayloadSchemaError>
    where
        P: CellPayload,
    {
        let kind = payload.kind();
        let payload = serde_json::to_value(payload)
            .map_err(|error| CellPayloadSchemaError::invalid(kind.as_str(), error))?;
        Ok(Self {
            kind: kind.as_str().to_owned(),
            payload,
        })
    }

    /// 按 registry 校验 envelope。
    pub fn validate(self) -> Result<Self, CellPayloadSchemaError> {
        let Some(entry) = CELL_PAYLOAD_REGISTRY
            .iter()
            .find(|entry| entry.kind.as_str() == self.kind)
        else {
            return Err(CellPayloadSchemaError::unknown_kind(self.kind));
        };
        let payload = (entry.validate)(entry.kind, self.payload)?;
        Ok(Self {
            kind: self.kind,
            payload,
        })
    }
}

impl CellPayloadSchemaError {
    pub(crate) fn message_source_required() -> Self {
        Self {
            kind: MessageCellPayload::KIND.as_str().to_owned(),
            message: "message payload 必须从包含 source 的 RenderCellRecord 投影".to_owned(),
        }
    }

    fn unknown_kind(kind: String) -> Self {
        Self {
            kind,
            message: "未注册 cell payload kind".to_owned(),
        }
    }

    fn invalid(kind: impl Into<String>, error: impl fmt::Display) -> Self {
        Self {
            kind: kind.into(),
            message: error.to_string(),
        }
    }

    /// 返回出错 payload kind。
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

impl fmt::Display for CellPayloadSchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for CellPayloadSchemaError {}

fn validate_payload<T>(
    kind: CellPayloadKind,
    payload: Value,
) -> Result<Value, CellPayloadSchemaError>
where
    T: CellPayload + DeserializeOwned,
{
    serde_json::from_value::<T>(payload.clone())
        .map(|_| payload)
        .map_err(|error| CellPayloadSchemaError::invalid(kind.as_str(), error))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn cell_payload_registry_covers_every_target_cell_kind() {
        let registry_kinds = CELL_PAYLOAD_REGISTRY
            .iter()
            .map(|entry| entry.kind.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let expected_kinds = EXPECTED_CELL_PAYLOAD_KINDS
            .iter()
            .map(|kind| kind.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(registry_kinds, expected_kinds);
        assert_eq!(CELL_PAYLOAD_REGISTRY.len(), expected_kinds.len());
    }

    #[test]
    fn cell_payload_registry_rejects_unknown_kind() {
        let error = CellPayloadEnvelope {
            kind: "unknown".to_owned(),
            payload: json!({}),
        }
        .validate()
        .expect_err("unknown kind must fail");

        assert_eq!(error.kind(), "unknown");
    }

    #[test]
    fn cell_payload_registry_rejects_missing_required_field() {
        let error = CellPayloadEnvelope {
            kind: DiffCellPayload::KIND.as_str().to_owned(),
            payload: json!({ "title": "Diff" }),
        }
        .validate()
        .expect_err("missing field must fail");

        assert_eq!(error.kind(), DiffCellPayload::KIND.as_str());
    }

    #[test]
    fn cell_payload_roundtrip_validates_multiple_schemas() {
        let message = MessageCellPayload {
            id: "message:test".to_owned(),
            source: "完成".to_owned(),
            source_kind: super::super::payloads::MessageSourceKindPayload::Markdown,
        };
        let usage = UsageCellPayload {
            id: "usage:test:payload".to_owned(),
            title: "Token".to_owned(),
            total_tokens: 42,
            total: "42 total".to_owned(),
            input_tokens: 20,
            input: "20 input".to_owned(),
            cache_read_input_tokens: 10,
            cache_read: "10 cached".to_owned(),
            cache_write_input_tokens: 2,
            cache_write: "2 written".to_owned(),
            output_tokens: 12,
            output: "12 output".to_owned(),
            reasoning_output_tokens: 0,
            reasoning: "0 reasoning".to_owned(),
        };

        assert_eq!(
            CellPayloadEnvelope::from_payload(&message)
                .expect("message envelope")
                .validate()
                .expect("message schema")
                .kind,
            MessageCellPayload::KIND.as_str()
        );
        assert_eq!(
            CellPayloadEnvelope::from_payload(&usage)
                .expect("usage envelope")
                .validate()
                .expect("usage schema")
                .kind,
            UsageCellPayload::KIND.as_str()
        );
    }
}
