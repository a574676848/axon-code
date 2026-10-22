//! Provider 可见工具输出构造。
//!
//! call_id 与 success 由强类型消息承载；本模块只负责有界 output 正文，
//! 并把缺字段和重试提示合并到同一条失败工具结果中。

use crate::AGENT_RUN_TOOL_ERROR_TYPE_MCP_UNAVAILABLE;

use super::{AgentRunToolResultReceipt, agent_run_tool_receipt_completed_from_facts};

const APPROX_BYTES_PER_TOKEN: u64 = 4;
/// 单条工具结果进入事件流前的固定内联预算（估算 token）。
///
/// 与上下文窗口无关：预算越大，越容易把一条工具结果撑成巨型事件，界面与
/// durable 流都为它买单。工具结果的完整正文一律落归档，模型与界面拿到的
/// 都是这一预算内的「头 + 尾 + 归档路径」。
pub const TOOL_RESULT_MAX_INLINE_TOKENS: u64 = 12_500;
/// 固定内联预算换算出的字节数。
pub const TOOL_RESULT_MAX_INLINE_BYTES: usize =
    (TOOL_RESULT_MAX_INLINE_TOKENS * APPROX_BYTES_PER_TOKEN) as usize;
/// 中段省略符。
pub const TOOL_RESULT_OMISSION_SEPARATOR: &str = "\n\n[...]\n\n";
/// 头尾保留：按 UTF-8 字节预算保留头尾，中段以省略符代替。
///
/// 预算本身也计入省略符与调用方随后追加的提示，因此返回值不会超过 `budget_bytes`。
#[must_use]
pub fn retain_head_tail_bytes(value: &str, budget_bytes: usize) -> String {
    if value.len() <= budget_bytes {
        return value.to_owned();
    }
    let separator = TOOL_RESULT_OMISSION_SEPARATOR;
    if budget_bytes <= separator.len() {
        return bounded_utf8_prefix(separator, budget_bytes);
    }
    let retained = budget_bytes - separator.len();
    let mut output = String::with_capacity(budget_bytes);
    output.push_str(&bounded_utf8_prefix(value, retained.div_ceil(2)));
    output.push_str(separator);
    output.push_str(&bounded_utf8_suffix(value, retained / 2));
    output
}

/// 省略提示：说明略过多少字节，并给出完整输出位置。
#[must_use]
pub fn tool_result_omission_notice(omitted_bytes: usize, archive_path: Option<&str>) -> String {
    match archive_path {
        Some(path) => format!("\n\n（已省略 {omitted_bytes} 字节；完整输出见 {path}）"),
        None => format!("\n\n（已省略 {omitted_bytes} 字节）"),
    }
}
const MAX_TOOL_METADATA_EDGE_BYTES: usize = 900;
const MAX_TOOL_NAME_CHARS: usize = 256;
const MAX_TOOL_STATUS_CHARS: usize = 64;
const MAX_TOOL_ERROR_TYPE_CHARS: usize = 128;
const MAX_FEEDBACK_FIELD_CHARS: usize = 256;
const TOOL_OUTPUT_TRUNCATION_MARKER: &str = "tool output truncated";
const TOOL_OUTPUT_TRUNCATION_SEPARATOR: &str = "\n... tool output truncated ...\n";
const TOOL_FEEDBACK_RETRY_HINT: &str = "Use canonical tool names; before retrying, include every missing required field and change failed arguments.";
const MCP_UNAVAILABLE_RETRY_HINT: &str = "Continue with built-in tools or another healthy MCP server. Retry the unavailable MCP only after its configuration, arguments, or health has changed.";
const TOOL_OUTPUT_ARCHIVE_PATH_METADATA_KEY: &str = "archivePath";
const TOOL_OUTPUT_ARCHIVE_READ_PATH_FIELD: &str = "readPath";
const TOOL_OUTPUT_ARCHIVE_READ_HINT: &str = "输出已截断；如需查看完整输出，请调用 builtin.read_file，并使用 output_ref.readPath 配合 offset/limit 分页读取。";

/// Provider 工具结果的模型可见截断策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRunToolOutputPolicy {
    max_output_bytes: usize,
}

impl AgentRunToolOutputPolicy {
    /// 单条工具结果的固定内联预算。
    ///
    /// 曾按上下文窗口的比例推导（窗口的 1/4）：窗口越大，单条结果允许越大，
    /// 一条 `builtin.search` 就能写出上百 MB 的事件。改为固定预算后，超出部分
    /// 一律进归档，模型与界面读同一份裁剪结果。
    #[must_use]
    pub const fn inline_budget() -> Self {
        Self {
            max_output_bytes: TOOL_RESULT_MAX_INLINE_BYTES,
        }
    }

    #[cfg(test)]
    pub(crate) const fn with_max_output_bytes(max_output_bytes: usize) -> Self {
        Self { max_output_bytes }
    }

    /// 返回单条 provider 工具结果的最大 UTF-8 字节预算。
    #[must_use]
    pub const fn max_output_bytes(self) -> usize {
        self.max_output_bytes
    }
}

impl Default for AgentRunToolOutputPolicy {
    fn default() -> Self {
        Self::inline_budget()
    }
}

/// 判断完整 Provider 可见工具结果是否超过当前模型预算。
#[must_use]
pub fn agent_run_tool_result_requires_archive(
    receipt: &AgentRunToolResultReceipt,
    policy: AgentRunToolOutputPolicy,
) -> bool {
    agent_run_tool_result_archive_text(receipt).len() > policy.max_output_bytes()
}

/// 序列化未截断的 Provider 可见工具结果，供完整输出归档使用。
#[must_use]
pub fn agent_run_tool_result_archive_text(receipt: &AgentRunToolResultReceipt) -> String {
    let feedback = tool_feedback(receipt);
    let metadata = receipt
        .metadata
        .as_ref()
        .map(provider_visible_metadata)
        .unwrap_or(serde_json::Value::Null);
    serialize(&output_payload(
        receipt,
        receipt.summary.clone(),
        metadata,
        feedback,
    ))
}

pub(super) fn tool_result_output(
    receipt: &AgentRunToolResultReceipt,
    policy: AgentRunToolOutputPolicy,
) -> String {
    let feedback = tool_feedback(receipt);
    let provider_metadata = receipt.metadata.as_ref().map(provider_visible_metadata);
    let full = output_payload(
        receipt,
        receipt.summary.clone(),
        provider_metadata.clone().unwrap_or(serde_json::Value::Null),
        feedback.clone(),
    );
    let serialized = serialize(&full);
    if serialized.len() <= policy.max_output_bytes() {
        return serialized;
    }

    let metadata = provider_metadata
        .as_ref()
        .map(serialize)
        .unwrap_or_else(|| "null".to_owned());
    let compact = compact_output_payload(
        receipt,
        truncated_metadata(&metadata, policy),
        feedback,
        policy,
    );
    let serialized = serialize(&compact);
    if serialized.len() <= policy.max_output_bytes() {
        return serialized;
    }

    serialize(&serde_json::json!({
        "tool": bounded_chars(&receipt.tool_name, MAX_TOOL_NAME_CHARS),
        "status": bounded_chars(&receipt.status, MAX_TOOL_STATUS_CHARS),
        "error_type": bounded_chars(
            receipt.error_type.as_deref().unwrap_or("none"),
            MAX_TOOL_ERROR_TYPE_CHARS,
        ),
        "summary": "",
        "session_id": receipt
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("sessionId"))
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "output_ref": output_ref_value(receipt, true),
        "archive_read_hint": archive_read_hint(receipt),
        "metadata": {
            "truncated": true,
            "marker": TOOL_OUTPUT_TRUNCATION_MARKER,
        },
    }))
}

pub(super) fn tool_result_success(receipt: &AgentRunToolResultReceipt) -> bool {
    agent_run_tool_receipt_completed_from_facts(receipt)
}

fn output_payload(
    receipt: &AgentRunToolResultReceipt,
    summary: String,
    metadata: serde_json::Value,
    feedback: Option<serde_json::Value>,
) -> serde_json::Value {
    let session_id = receipt
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("sessionId"))
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let mut payload = serde_json::json!({
        "tool": receipt.tool_name,
        "status": receipt.status,
        "error_type": receipt.error_type.as_deref().unwrap_or("none"),
        "remote_trigger_allowed": receipt.remote_trigger_allowed,
        "filesystem_write_performed": receipt.filesystem_write_performed,
        "summary": summary,
        "metadata": metadata,
        "session_id": session_id,
        "output_ref": output_ref_value(receipt, false),
        "archive_read_hint": archive_read_hint(receipt),
    });
    if let Some(feedback) = feedback {
        payload["feedback"] = feedback;
    }
    payload
}

fn tool_feedback(receipt: &AgentRunToolResultReceipt) -> Option<serde_json::Value> {
    if tool_result_success(receipt) {
        return None;
    }

    let retry_hint =
        if receipt.error_type.as_deref() == Some(AGENT_RUN_TOOL_ERROR_TYPE_MCP_UNAVAILABLE) {
            MCP_UNAVAILABLE_RETRY_HINT
        } else {
            TOOL_FEEDBACK_RETRY_HINT
        };
    Some(serde_json::json!({
        "missing_fields": missing_fields(receipt),
        "retry_hint": retry_hint,
    }))
}

fn missing_fields(receipt: &AgentRunToolResultReceipt) -> Vec<String> {
    let mut fields = receipt
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("missingFields"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(|field| bounded_chars(field, MAX_FEEDBACK_FIELD_CHARS))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if fields.is_empty()
        && let Some(field) = missing_field_from_summary(&receipt.summary)
    {
        fields.push(bounded_chars(field, MAX_FEEDBACK_FIELD_CHARS));
    }
    fields
}

fn missing_field_from_summary(summary: &str) -> Option<&str> {
    const MISSING_FIELD_MARKER: &str = "缺少 ";
    let index = summary.find(MISSING_FIELD_MARKER)?;
    let field = summary[index + MISSING_FIELD_MARKER.len()..]
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '，' | '.' | '。'))
        .next()
        .unwrap_or("")
        .trim();
    (!field.is_empty()).then_some(field)
}

fn truncated_metadata(metadata: &str, policy: AgentRunToolOutputPolicy) -> serde_json::Value {
    let original_bytes = metadata.len();
    let edge_bytes = MAX_TOOL_METADATA_EDGE_BYTES.min(policy.max_output_bytes() / 8);
    let head = bounded_utf8_prefix(metadata, edge_bytes);
    let tail = bounded_utf8_suffix(metadata, edge_bytes);
    serde_json::json!({
        "truncated": true,
        "marker": TOOL_OUTPUT_TRUNCATION_MARKER,
        "original_bytes": original_bytes,
        "head": head,
        "tail": tail,
    })
}

fn provider_visible_metadata(metadata: &serde_json::Value) -> serde_json::Value {
    let mut metadata = metadata.clone();
    if let Some(object) = metadata.as_object_mut() {
        object.remove("modelMessages");
        object.remove(TOOL_OUTPUT_ARCHIVE_PATH_METADATA_KEY);
    }
    metadata
}

fn bounded_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn bounded_utf8_prefix(value: &str, max_bytes: usize) -> String {
    value
        .char_indices()
        .take_while(|(index, ch)| index.saturating_add(ch.len_utf8()) <= max_bytes)
        .map(|(_, ch)| ch)
        .collect()
}

fn bounded_utf8_suffix(value: &str, max_bytes: usize) -> String {
    let mut retained_bytes = 0usize;
    let mut chars = value
        .chars()
        .rev()
        .take_while(|ch| {
            retained_bytes = retained_bytes.saturating_add(ch.len_utf8());
            retained_bytes <= max_bytes
        })
        .collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return value.to_owned();
    }
    let separator_chars = TOOL_OUTPUT_TRUNCATION_SEPARATOR.chars().count();
    if max_chars <= separator_chars {
        return bounded_chars(TOOL_OUTPUT_TRUNCATION_SEPARATOR, max_chars);
    }
    let retained = max_chars - separator_chars;
    let head_len = retained.div_ceil(2);
    let tail_len = retained / 2;
    let mut output = chars[..head_len].iter().collect::<String>();
    output.push_str(TOOL_OUTPUT_TRUNCATION_SEPARATOR);
    output.extend(chars[chars.len() - tail_len..].iter());
    output
}

fn compact_output_payload(
    receipt: &AgentRunToolResultReceipt,
    metadata: serde_json::Value,
    feedback: Option<serde_json::Value>,
    policy: AgentRunToolOutputPolicy,
) -> serde_json::Value {
    let total_chars = receipt.summary.chars().count();
    let mut low = 0usize;
    let mut high = total_chars;
    let mut best = String::new();
    while low <= high {
        let candidate_chars = low + (high - low) / 2;
        let candidate = truncate_middle(&receipt.summary, candidate_chars);
        let payload = output_payload(
            receipt,
            candidate.clone(),
            metadata.clone(),
            feedback.clone(),
        );
        if serialize(&payload).len() <= policy.max_output_bytes() {
            best = candidate;
            low = candidate_chars.saturating_add(1);
        } else if candidate_chars == 0 {
            break;
        } else {
            high = candidate_chars - 1;
        }
    }
    output_payload(receipt, best, metadata, feedback)
}

fn minimal_output_ref(output_ref: &axon_domain::OutputRef) -> axon_domain::OutputRef {
    let mut projection = output_ref.clone();
    projection.preview.clear();
    projection
}

fn output_ref_value(receipt: &AgentRunToolResultReceipt, clear_preview: bool) -> serde_json::Value {
    let Some(output_ref) = receipt.output_ref.as_ref() else {
        return serde_json::Value::Null;
    };
    let projected = if clear_preview {
        minimal_output_ref(output_ref)
    } else {
        output_ref.clone()
    };
    let mut value = serde_json::to_value(projected).unwrap_or_default();
    if let Some(path) = receipt
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(TOOL_OUTPUT_ARCHIVE_PATH_METADATA_KEY))
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        && let Some(object) = value.as_object_mut()
    {
        object.insert(
            TOOL_OUTPUT_ARCHIVE_READ_PATH_FIELD.to_owned(),
            serde_json::Value::String(path.to_owned()),
        );
    }
    value
}

fn archive_read_hint(receipt: &AgentRunToolResultReceipt) -> Option<&'static str> {
    receipt.output_ref.as_ref().and_then(|_| {
        receipt
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get(TOOL_OUTPUT_ARCHIVE_PATH_METADATA_KEY))
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(|_| TOOL_OUTPUT_ARCHIVE_READ_HINT)
    })
}

fn serialize(value: &serde_json::Value) -> String {
    value.to_string()
}
