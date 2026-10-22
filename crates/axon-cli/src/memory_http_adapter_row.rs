//! 记忆管理 Gateway 的请求行解析与对外行视图。

use axon_agent_runtime::MemorySqliteRecord;
use axon_domain::MemoryState;
use serde::Deserialize;
use serde_json::Value;

/// 单条写入请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MemoryWriteItem {
    /// 五值稳定类型。
    pub memory_kind: String,
    /// 记忆摘要。
    pub summary: String,
    /// 证据引用。
    pub evidence: String,
    /// 仓库记忆必填的文件夹目录；Personal 类型不得提供。
    #[serde(default)]
    pub workspace_path: Option<String>,
    /// 在批量条目内声明的替代目标；id 是 u64，优先用十进制字符串传输。
    #[serde(default, deserialize_with = "de_memory_ids")]
    pub supersedes_memory_ids: Vec<String>,
}

/// 接受十进制字符串或数字的 id 列表。
///
/// `memory_id` 是 u64，超出 JS 安全整数区间；界面必须以字符串传输，数字形态保留
/// 给 CLI、curl 等能精确表示 u64 的调用方。
fn de_memory_ids<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Option::<Vec<serde_json::Value>>::deserialize(deserializer)?.unwrap_or_default();
    values
        .into_iter()
        .map(|value| match value {
            serde_json::Value::String(text) => Ok(text),
            serde_json::Value::Number(number) => Ok(number.to_string()),
            _ => Err(serde::de::Error::custom(
                "supersedesMemoryIds 必须是十进制 id 字符串或数字",
            )),
        })
        .collect()
}

/// 批量写入包装体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MemoryWriteBatch {
    records: Vec<MemoryWriteItem>,
}

/// 请求体解析失败。
#[derive(Debug)]
pub(crate) struct WriteBodyError(pub String);

/// 解析写入请求体：支持单条对象、`{"records": [...]}` 与裸数组三种形态。
pub(crate) fn parse_write_body(body: &[u8]) -> Result<Vec<MemoryWriteItem>, WriteBodyError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| WriteBodyError(format!("Memory 写入请求不是合法 JSON：{error}")))?;
    match value {
        Value::Array(_) => serde_json::from_value::<Vec<MemoryWriteItem>>(value)
            .map_err(|error| WriteBodyError(format!("Memory 写入请求无效：{error}"))),
        Value::Object(ref map) if map.contains_key("records") => {
            serde_json::from_value::<MemoryWriteBatch>(value)
                .map(|batch| batch.records)
                .map_err(|error| WriteBodyError(format!("Memory 写入请求无效：{error}")))
        }
        Value::Object(_) => serde_json::from_value::<MemoryWriteItem>(value)
            .map(|item| vec![item])
            .map_err(|error| WriteBodyError(format!("Memory 写入请求无效：{error}"))),
        _ => Err(WriteBodyError(
            "Memory 写入请求必须是对象、数组或 records 包装体".to_owned(),
        )),
    }
}

/// 构造对外记忆行视图；字段名与 CLI JSON 保持一致。
///
/// 只暴露用户可见事实：命名空间与仓库引用属于内部派生，不下发到界面层。
pub(crate) fn memory_view(record: &MemorySqliteRecord) -> Value {
    let source = record.record.source_trace.as_ref();
    serde_json::json!({
        // memory_id 是 u64，超出 JS 安全整数区间；一律以十进制字符串下发。
        "memoryId": record.record.id.0.to_string(),
        "memoryKind": record.memory_kind.as_str(),
        "state": state_code(record.record.state),
        "memorySummary": source.map(|item| item.summary.clone()),
        "contentRef": record.record.content_ref,
        "createdAtMs": record.created_at_ms,
        "updatedAtMs": record.updated_at_ms,
        "lastActiveAtMs": record.last_active_at_ms,
        "archivedAtMs": record.archived_at_ms,
        "confidenceBasisPoints": record.confidence_basis_points,
        "importanceBasisPoints": record.importance_basis_points,
        "decayedScoreBasisPoints": axon_agent_runtime::decayed_score_basis_points(
            record,
            now_ms(),
        ),
        "halfLifeDays": axon_agent_runtime::MEMORY_DEFAULT_HALF_LIFE_DAYS,
        "scope": if record.user_ref.is_some() { "personal" } else { "workspace" },
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn state_code(state: MemoryState) -> &'static str {
    match state {
        MemoryState::Weak => "weak",
        MemoryState::Strong => "strong",
        MemoryState::Quarantined => "quarantined",
        MemoryState::Tombstone => "tombstone",
    }
}
