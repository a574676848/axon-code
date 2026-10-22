use std::{
    collections::{BTreeSet, HashSet},
    path::PathBuf,
};

use serde::Serialize;

use crate::{
    AgentModelLocalImageRef, AgentModelMessage, AgentModelToolCallMetadata, AgentTurnToolDecision,
    ProviderToolCall,
};
use axon_domain::OutputRef;

#[path = "agent_run_tool_result_wire_facts.rs"]
mod agent_run_tool_result_wire_facts;
pub use agent_run_tool_result_wire_facts::{
    AgentRunToolDecisionFactsInput, agent_run_changed_file_paths_from_facts,
    agent_run_tool_decision_from_facts, agent_run_tool_receipt_completed_from_facts,
    agent_run_tool_receipt_write_scan_from_facts,
};
#[path = "agent_run_tool_result_output.rs"]
mod agent_run_tool_result_output;
pub use agent_run_tool_result_output::{
    AgentRunToolOutputPolicy, TOOL_RESULT_MAX_INLINE_BYTES, TOOL_RESULT_MAX_INLINE_TOKENS,
    TOOL_RESULT_OMISSION_SEPARATOR, agent_run_tool_result_archive_text,
    agent_run_tool_result_requires_archive, retain_head_tail_bytes, tool_result_omission_notice,
};
use agent_run_tool_result_output::{tool_result_output, tool_result_success};
#[path = "agent_run_tool_result_wire_provider_receipt.rs"]
mod agent_run_tool_result_wire_provider_receipt;
pub use agent_run_tool_result_wire_provider_receipt::{
    agent_run_provider_receipt_status_name, agent_run_tool_receipt_facts_to_provider_receipt_json,
    agent_run_tool_receipt_to_execution_outcome, agent_run_tool_receipt_to_provider_receipt,
    agent_run_tool_receipts_to_provider_receipt_json,
};

const TOOL_RESULT_LOCAL_IMAGES_FIELD: &str = "localImages";
const TOOL_RESULT_LOCAL_IMAGE_PATH_FIELD: &str = "path";
const TOOL_RESULT_LOCAL_IMAGE_MIME_FIELD: &str = "mimeType";
const TOOL_RESULT_IMAGE_PROMPT: &str = "工具返回了本地图片，请结合图片像素继续处理当前任务。";

/// Provider tool result 回灌到下一轮模型消息所需的 runtime receipt 投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunToolResultReceipt {
    /// Provider 返回的 tool call id。
    pub provider_call_id: String,
    /// 实际执行的工具名。
    pub tool_name: String,
    /// 工具执行状态。
    pub status: String,
    /// 发送给模型的有界摘要。
    pub summary: String,
    /// 工具执行产生的结构化元数据。
    pub metadata: Option<serde_json::Value>,
    /// 工具执行产生的完整输出引用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<OutputRef>,
    /// 失败或阻断类型。
    pub error_type: Option<String>,
    /// 本次工具执行是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 本次工具执行是否写入了文件系统。
    pub filesystem_write_performed: bool,
}

/// Tool receipt facts exposed by an adapter-local tool executor.
pub trait AgentRunToolResultReceiptFacts {
    /// Provider returned tool call id.
    fn provider_call_id(&self) -> &str;
    /// Executed tool name.
    fn tool_name(&self) -> &str;
    /// Execution status.
    fn status(&self) -> &str;
    /// Bounded execution summary.
    fn summary(&self) -> &str;
    /// Structured execution metadata.
    fn metadata(&self) -> Option<&serde_json::Value>;
    /// 完整输出引用。
    fn output_ref(&self) -> Option<&OutputRef>;
    /// Failure or block type.
    fn error_type(&self) -> Option<&str>;
    /// Whether remote trigger was allowed.
    fn remote_trigger_allowed(&self) -> bool;
    /// Whether filesystem writes were performed.
    fn filesystem_write_performed(&self) -> bool;
}

impl AgentRunToolResultReceiptFacts for AgentRunToolResultReceipt {
    fn provider_call_id(&self) -> &str {
        &self.provider_call_id
    }

    fn tool_name(&self) -> &str {
        &self.tool_name
    }

    fn status(&self) -> &str {
        &self.status
    }

    fn summary(&self) -> &str {
        &self.summary
    }

    fn metadata(&self) -> Option<&serde_json::Value> {
        self.metadata.as_ref()
    }

    fn output_ref(&self) -> Option<&OutputRef> {
        self.output_ref.as_ref()
    }

    fn error_type(&self) -> Option<&str> {
        self.error_type.as_deref()
    }

    fn remote_trigger_allowed(&self) -> bool {
        self.remote_trigger_allowed
    }

    fn filesystem_write_performed(&self) -> bool {
        self.filesystem_write_performed
    }
}

/// Input for projecting executed tool receipts into a turn-runtime tool decision.
pub struct AgentRunToolDecisionInput<'a> {
    /// Provider tool calls returned by the current turn.
    pub tool_calls: &'a [ProviderToolCall],
    /// Runtime tool receipts produced by the local executor.
    pub receipts: Vec<AgentRunToolResultReceipt>,
    /// Provider reasoning content carried into assistant tool-call messages.
    pub reasoning_content: Option<&'a str>,
    /// 当前模型对应的工具输出预算。
    pub output_policy: AgentRunToolOutputPolicy,
}

/// Project adapter-local tool receipt facts into runtime-owned receipt inputs.
#[must_use]
pub fn agent_run_tool_result_receipts_from_facts<R>(
    receipts: &[R],
) -> Vec<AgentRunToolResultReceipt>
where
    R: AgentRunToolResultReceiptFacts,
{
    receipts
        .iter()
        .map(agent_run_tool_result_receipt_from_facts)
        .collect()
}

/// Project one adapter-local tool receipt into a runtime-owned receipt input.
#[must_use]
pub fn agent_run_tool_result_receipt_from_facts<R>(receipt: &R) -> AgentRunToolResultReceipt
where
    R: AgentRunToolResultReceiptFacts + ?Sized,
{
    AgentRunToolResultReceipt {
        provider_call_id: receipt.provider_call_id().to_owned(),
        tool_name: receipt.tool_name().to_owned(),
        status: receipt.status().to_owned(),
        summary: receipt.summary().to_owned(),
        metadata: receipt.metadata().cloned(),
        output_ref: receipt.output_ref().cloned(),
        error_type: receipt.error_type().map(ToOwned::to_owned),
        remote_trigger_allowed: receipt.remote_trigger_allowed(),
        filesystem_write_performed: receipt.filesystem_write_performed(),
    }
}

/// JSON projection of provider tool execution receipts for headless run events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunProviderToolReceiptJson {
    /// Dispatch order within the local tool execution batch.
    pub dispatch_order: usize,
    /// Runtime tool call id.
    pub tool_call_id: u64,
    /// Provider tool call id.
    pub provider_call_id: String,
    /// Tool name.
    pub tool_name: String,
    /// Runtime execution request id.
    pub execution_request_id: u64,
    /// Stable provider receipt status.
    pub status: String,
    /// Bounded execution summary.
    pub bounded_summary: String,
    /// Original execution metadata.
    pub metadata: Option<serde_json::Value>,
    /// 完整输出引用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<OutputRef>,
    /// Failure or blocking type.
    pub error_type: Option<String>,
    /// Whether the tool execution allowed remote trigger.
    pub remote_trigger_allowed: bool,
    /// Whether the tool execution wrote to the filesystem.
    pub filesystem_write_performed: bool,
    /// Completion timestamp in milliseconds.
    pub completed_at_ms: u64,
}

/// Runtime scan of write-related facts across one batch of tool receipts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunToolReceiptWriteScan {
    /// Whether any receipt wrote to the filesystem.
    pub has_filesystem_write: bool,
    /// Index of the last receipt that wrote to the filesystem.
    pub last_write_receipt_index: Option<usize>,
    /// Whether any write receipt carried post-apply diagnostics errors.
    pub post_apply_diagnostics_errors_detected: bool,
    /// Changed file paths extracted from write receipt metadata.
    pub changed_files: Vec<PathBuf>,
}

/// 将 provider tool calls 与本机 tool receipts 组装成下一轮 provider 消息。
///
/// **批次相邻性不变式**：OpenAI 兼容协议要求 `assistant(tool_calls)` 之后紧跟同一批次的
/// 全部 `tool` 消息，任何非 `tool` 消息插在同批 `tool` 之间都会被网关整条拒绝
/// （实测 MaaS 返回 `400 / 400001 invalid_request_error`）。因此本函数分两段发射：
/// 先发完整批次的 `tool` 结果，再统一追加绑定在各 receipt 上的附注（mailbox 运行时注记、
/// 工具图片提示）；附注之间的相对顺序保持 receipt 顺序不变。
pub fn agent_run_tool_result_messages(
    tool_calls: &[ProviderToolCall],
    receipts: &[AgentRunToolResultReceipt],
    reasoning_content: Option<&str>,
    output_policy: AgentRunToolOutputPolicy,
) -> Result<Vec<AgentModelMessage>, String> {
    validate_tool_receipt_pairing(tool_calls, receipts)?;

    let mut messages = vec![assistant_tool_calls_message(tool_calls, reasoning_content)];
    let mut annotations = Vec::new();
    for receipt in receipts {
        messages.push(
            AgentModelMessage::tool_result(
                &receipt.provider_call_id,
                tool_result_output(receipt, output_policy),
                tool_result_success(receipt),
            )
            .with_output_ref_if_present(receipt.output_ref.as_ref()),
        );
        annotations.extend(receipt_model_messages(receipt));
        let images = tool_result_local_images(receipt);
        if !images.is_empty() {
            annotations.push(
                AgentModelMessage::cache_excluded("user", TOOL_RESULT_IMAGE_PROMPT)
                    .with_local_images(images),
            );
        }
    }
    messages.extend(annotations);
    Ok(messages)
}

fn receipt_model_messages(receipt: &AgentRunToolResultReceipt) -> Vec<AgentModelMessage> {
    receipt
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("modelMessages"))
        .cloned()
        .and_then(|messages| serde_json::from_value(messages).ok())
        .unwrap_or_default()
}

/// Project tool execution receipts into the decision consumed by `AgentTurnRuntime`.
///
/// Runtime owns provider message projection; callers still own environment-specific tool execution
/// and deferred model-tool loading.
pub fn agent_run_tool_decision(
    input: AgentRunToolDecisionInput<'_>,
) -> Result<AgentTurnToolDecision<AgentRunToolResultReceipt>, String> {
    let AgentRunToolDecisionInput {
        tool_calls,
        receipts,
        reasoning_content,
        output_policy,
    } = input;

    // UQ-008 挂起收口：若存在挂起的提问工具调用，轮次经 AwaitExternal 收口，不再继续发起 provider 请求
    let has_suspended_question = receipts.iter().any(|receipt| {
        let is_question_tool = receipt.tool_name == crate::ASK_USER_QUESTION_TOOL_NAME
            || receipt.tool_name == crate::ASK_USER_QUESTION_TOOL_QUALIFIED_NAME;

        if !is_question_tool {
            return false;
        }

        serde_json::from_str::<serde_json::Value>(&receipt.summary)
            .ok()
            .and_then(|p| p.get("suspended").and_then(serde_json::Value::as_bool))
            .unwrap_or(false)
    });

    if has_suspended_question {
        return Ok(AgentTurnToolDecision::AwaitExternal { receipts });
    }

    let next_messages =
        agent_run_tool_result_messages(tool_calls, &receipts, reasoning_content, output_policy)?;
    Ok(AgentTurnToolDecision::Continue {
        receipts,
        next_messages,
    })
}

/// 判断工具回执是否包含 post-apply diagnostics 错误。
#[must_use]
pub fn agent_run_receipts_have_post_apply_diagnostics_errors(
    receipts: &[AgentRunToolResultReceipt],
) -> bool {
    agent_run_tool_receipt_write_scan(receipts).post_apply_diagnostics_errors_detected
}

/// 从工具回执 metadata 中提取本轮写入路径。
#[must_use]
pub fn agent_run_changed_file_paths_from_receipts(
    receipts: &[AgentRunToolResultReceipt],
) -> Vec<PathBuf> {
    agent_run_tool_receipt_write_scan(receipts).changed_files
}

/// Scan write-related facts from tool receipts for verify/self-correction loops.
#[must_use]
pub fn agent_run_tool_receipt_write_scan(
    receipts: &[AgentRunToolResultReceipt],
) -> AgentRunToolReceiptWriteScan {
    let mut has_filesystem_write = false;
    let mut last_write_receipt_index = None;
    let mut post_apply_diagnostics_errors_detected = false;
    let mut changed_files = BTreeSet::new();

    for (index, receipt) in receipts.iter().enumerate() {
        if !receipt.filesystem_write_performed {
            continue;
        }
        has_filesystem_write = true;
        last_write_receipt_index = Some(index);
        let Some(metadata) = receipt.metadata.as_ref() else {
            continue;
        };
        if metadata
            .get("postApplyDiagnosticsErrors")
            .and_then(serde_json::Value::as_array)
            .is_some()
        {
            post_apply_diagnostics_errors_detected = true;
        }
        if let Some(path) = metadata
            .get("file_path")
            .or_else(|| metadata.get("path"))
            .and_then(serde_json::Value::as_str)
        {
            changed_files.insert(PathBuf::from(path));
        }
        if let Some(affected_paths) = metadata
            .get("affectedPaths")
            .and_then(serde_json::Value::as_array)
        {
            changed_files.extend(
                affected_paths
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(PathBuf::from),
            );
        }
    }

    AgentRunToolReceiptWriteScan {
        has_filesystem_write,
        last_write_receipt_index,
        post_apply_diagnostics_errors_detected,
        changed_files: changed_files.into_iter().collect(),
    }
}

fn tool_result_local_images(receipt: &AgentRunToolResultReceipt) -> Vec<AgentModelLocalImageRef> {
    receipt
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(TOOL_RESULT_LOCAL_IMAGES_FIELD))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|image| {
            let path = image
                .get(TOOL_RESULT_LOCAL_IMAGE_PATH_FIELD)
                .and_then(serde_json::Value::as_str)?;
            let mime_type = image
                .get(TOOL_RESULT_LOCAL_IMAGE_MIME_FIELD)
                .and_then(serde_json::Value::as_str)?;
            (!path.trim().is_empty() && mime_type.starts_with("image/")).then(|| {
                AgentModelLocalImageRef {
                    path: path.to_owned(),
                    mime_type: mime_type.to_owned(),
                    byte_len: None,
                    sha256: None,
                    archive_ref: None,
                    source_ref: None,
                }
            })
        })
        .collect()
}

fn validate_tool_receipt_pairing(
    tool_calls: &[ProviderToolCall],
    receipts: &[AgentRunToolResultReceipt],
) -> Result<(), String> {
    if tool_calls.len() != receipts.len() {
        return Err(format!(
            "provider tool result 数量不匹配：calls={} receipts={}",
            tool_calls.len(),
            receipts.len()
        ));
    }

    let mut seen_call_ids = HashSet::new();
    let mut seen_receipt_ids = HashSet::new();
    for (index, (tool_call, receipt)) in tool_calls.iter().zip(receipts).enumerate() {
        let call_id = non_empty_tool_call_id(&tool_call.id, index, "call")?;
        let receipt_id = non_empty_tool_call_id(&receipt.provider_call_id, index, "receipt")?;
        if !seen_call_ids.insert(call_id.to_owned()) {
            return Err(format!("provider tool call_id 重复：{call_id}"));
        }
        if !seen_receipt_ids.insert(receipt_id.to_owned()) {
            return Err(format!("provider tool receipt call_id 重复：{receipt_id}"));
        }
        if call_id != receipt_id {
            return Err(format!(
                "provider tool result 与 call_id 不匹配：call={call_id} receipt={receipt_id}"
            ));
        }
    }
    Ok(())
}

fn non_empty_tool_call_id<'a>(
    value: &'a str,
    index: usize,
    label: &str,
) -> Result<&'a str, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("provider tool {label} call_id 为空：index={index}"));
    }
    Ok(trimmed)
}

/// 将 provider tool calls 投影为下一轮模型可识别的 assistant tool-call 消息。
#[must_use]
pub fn agent_run_tool_calls_message(
    tool_calls: &[ProviderToolCall],
    reasoning_content: Option<&str>,
) -> AgentModelMessage {
    assistant_tool_calls_message(tool_calls, reasoning_content)
}

fn assistant_tool_calls_message(
    tool_calls: &[ProviderToolCall],
    reasoning_content: Option<&str>,
) -> AgentModelMessage {
    let message = AgentModelMessage::text("assistant", "").with_tool_calls(
        tool_calls
            .iter()
            .map(|tool_call| AgentModelToolCallMetadata {
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                input: tool_call.input.clone(),
            })
            .collect(),
    );
    match reasoning_content {
        Some(reasoning_content) if !reasoning_content.trim().is_empty() => {
            message.with_openai_reasoning_content(reasoning_content)
        }
        _ => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentRunToolDecisionInput;

    #[test]
    fn test_suspended_question_yields_await_external() {
        let tool_calls = vec![ProviderToolCall {
            id: "call-1".to_string(),
            name: "builtin.ask_user_question".to_string(),
            input: serde_json::json!({
                "questions": [{"id": "q1", "question": "确认？"}]
            })
            .into(),
        }];

        let receipts = vec![AgentRunToolResultReceipt {
            provider_call_id: "call-1".to_string(),
            tool_name: "builtin.ask_user_question".to_string(),
            status: "completed".to_string(),
            summary: serde_json::json!({
                "schemaVersion": 1,
                "kind": "ask_user_question",
                "pending": true,
                "suspended": true,
                "callId": "call-1",
                "notice": "等待超时已挂起"
            })
            .to_string(),
            metadata: None,
            output_ref: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
        }];

        let decision = agent_run_tool_decision(AgentRunToolDecisionInput {
            tool_calls: &tool_calls,
            receipts,
            reasoning_content: None,
            output_policy: AgentRunToolOutputPolicy::default(),
        })
        .expect("决策投影成功");

        match decision {
            AgentTurnToolDecision::AwaitExternal { receipts } => {
                assert_eq!(receipts.len(), 1);
                assert_eq!(receipts[0].tool_name, "builtin.ask_user_question");
            }
            AgentTurnToolDecision::Continue { .. } => panic!("期望 AwaitExternal，得到 Continue"),
        }
    }
}
