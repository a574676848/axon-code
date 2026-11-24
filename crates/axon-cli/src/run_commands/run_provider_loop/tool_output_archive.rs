//! Provider turn 工具结果的完整输出归档。

use axon_agent_runtime::{
    AgentRunToolOutputPolicy, AgentRunToolResultReceipt, CONTENT_ARCHIVE_RESULT_STREAM,
    ContentArchiveRuntime, agent_run_tool_result_archive_text,
    agent_run_tool_result_receipt_from_facts, agent_run_tool_result_requires_archive,
    tool_output_source_trace_from_dispatch_name,
};

use super::{RunRequest, RunToolReceipt};

const TOOL_METADATA_ARCHIVE_REF_KEY: &str = "archiveRef";
const TOOL_RECEIPT_BLOCKED_STATUS: &str = "blocked";
const TOOL_RECEIPT_BLOCKED_ERROR_TYPE: &str = "blocked";
const TOOL_OUTPUT_ARCHIVE_FAILURE_SUMMARY: &str = "工具完整输出归档失败，已阻断回执";
const TOOL_OUTPUT_ARCHIVE_FAILED_METADATA_KEY: &str = "archiveFailed";
const TOOL_OUTPUT_ARCHIVE_MISSING_REF_ERROR: &str = "归档未生成 OutputRef";
const TOOL_OUTPUT_METADATA_OBJECT_ERROR: &str = "工具回执 metadata 必须是对象";

pub(super) fn archive_oversized_tool_receipts(
    request: &RunRequest,
    receipts: &mut [RunToolReceipt],
) {
    let output_policy = AgentRunToolOutputPolicy::inline_budget();
    for receipt in receipts {
        if receipt.output_ref.is_some() {
            continue;
        }
        let runtime_receipt = agent_run_tool_result_receipt_from_facts(receipt);
        if !agent_run_tool_result_requires_archive(&runtime_receipt, output_policy) {
            continue;
        }
        if let Err(error) =
            archive_oversized_tool_receipt(request, receipt, &runtime_receipt, output_policy)
        {
            block_tool_receipt_for_archive_failure(receipt, &error);
        }
    }
}

fn archive_oversized_tool_receipt(
    request: &RunRequest,
    receipt: &mut RunToolReceipt,
    runtime_receipt: &AgentRunToolResultReceipt,
    output_policy: AgentRunToolOutputPolicy,
) -> Result<(), String> {
    let archive_root = crate::runtime_paths::runtime_axon_data_path_static(&request.config_path)
        .map_err(ToOwned::to_owned)?;
    let archive_text = agent_run_tool_result_archive_text(runtime_receipt);
    let preview = bounded_bytes_preview(&receipt.summary, output_policy.max_output_bytes());
    let source_trace = tool_output_source_trace_from_dispatch_name(&receipt.tool_name);
    let archived = ContentArchiveRuntime::default()
        .archive_text_with_preview(
            &archive_root,
            CONTENT_ARCHIVE_RESULT_STREAM,
            &archive_text,
            &source_trace,
            &preview,
            true,
        )
        .map_err(|error| error.to_string())?;
    let output_ref = archived
        .output_ref
        .ok_or_else(|| TOOL_OUTPUT_ARCHIVE_MISSING_REF_ERROR.to_owned())?;
    receipt.summary.clone_from(&output_ref.preview);
    attach_archived_tool_output(receipt, output_ref, &archive_root)
}

fn bounded_bytes_preview(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.trim().to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut preview = value[..end].trim().to_owned();
    preview.push_str("...");
    preview
}

fn attach_archived_tool_output(
    receipt: &mut RunToolReceipt,
    output_ref: axon_domain::OutputRef,
    archive_root: &std::path::Path,
) -> Result<(), String> {
    let output_ref_value = serde_json::to_value(&output_ref).map_err(|error| error.to_string())?;
    let metadata = receipt
        .metadata
        .get_or_insert_with(|| serde_json::json!({}));
    let metadata = metadata
        .as_object_mut()
        .ok_or_else(|| TOOL_OUTPUT_METADATA_OBJECT_ERROR.to_owned())?;
    metadata.insert(TOOL_METADATA_ARCHIVE_REF_KEY.to_owned(), output_ref_value);
    if let Some(archive_ref) = output_ref.archive_ref.as_ref() {
        metadata.insert(
            "archivePath".to_owned(),
            serde_json::json!(
                archive_root
                    .join(&archive_ref.relative_path)
                    .display()
                    .to_string()
            ),
        );
    }
    receipt.output_ref = Some(output_ref);
    Ok(())
}

fn block_tool_receipt_for_archive_failure(receipt: &mut RunToolReceipt, error: &str) {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        TOOL_OUTPUT_ARCHIVE_FAILED_METADATA_KEY.to_owned(),
        serde_json::Value::Bool(true),
    );
    receipt.status = TOOL_RECEIPT_BLOCKED_STATUS.to_owned();
    receipt.summary = format!("{TOOL_OUTPUT_ARCHIVE_FAILURE_SUMMARY}：{error}");
    receipt.metadata = Some(serde_json::Value::Object(metadata));
    receipt.error_type = Some(TOOL_RECEIPT_BLOCKED_ERROR_TYPE.to_owned());
    receipt.remote_trigger_allowed = false;
    receipt.output_ref = None;
}
