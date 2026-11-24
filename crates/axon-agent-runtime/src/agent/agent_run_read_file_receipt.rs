//! read_file 工具回执投影。

use crate::{
    AgentRunToolReceiptProjection, ProviderToolCall, ReadFileToolExecutionReceipt,
    agent_run_completed_tool_receipt,
};

/// Project a read_file execution receipt into the provider-visible tool receipt contract.
#[must_use]
pub fn agent_run_read_file_tool_receipt(
    tool_call: &ProviderToolCall,
    receipt: &ReadFileToolExecutionReceipt,
) -> AgentRunToolReceiptProjection {
    let hashline = read_file_hashline_metadata(receipt);
    let metadata = serde_json::json!({
        "kind": "read_file",
        "path": receipt.path,
        "contentKind": receipt.content_kind.as_str(),
        "truncated": receipt.truncated,
        "page": {
            "lineStart": receipt.line_start,
            "lineEnd": receipt.line_end,
            "totalLines": receipt.total_lines,
            "nextOffset": receipt.next_offset,
            "returnedBytes": receipt.returned_bytes,
            "truncationReason": receipt.truncation_reason,
        },
        "originalChars": receipt.original_chars,
        "filteredChars": receipt.filtered_chars,
        "contentSha256": receipt.content_sha256,
        "hashline": hashline,
        "seenLines": receipt.seen_lines,
        "filter": {
            "level": receipt.filter_level_applied.map(axon_output_filter::lsp_symbol_filter::LspFilterLevel::as_str),
            "symbolsExtracted": receipt.symbols_extracted,
            "lspUsed": receipt.lsp_used,
            "tokenSavings": {
                "originalTokens": receipt.token_savings.original_tokens,
                "filteredTokens": receipt.token_savings.filtered_tokens,
                "savingsPct": receipt.token_savings.savings_pct,
            }
        },
    });
    agent_run_completed_tool_receipt(
        tool_call,
        agent_run_read_file_tool_receipt_summary(receipt),
        Some(metadata),
        receipt.filesystem_write_performed,
    )
}

/// Project a read_file execution receipt into a bounded provider-visible summary.
#[must_use]
pub fn agent_run_read_file_tool_receipt_summary(receipt: &ReadFileToolExecutionReceipt) -> String {
    let header = format!(
        "read_file path={} content_kind={} hashline={} lines={}-{} total_lines={} next_offset={} bytes={} truncated={} filter_level={}",
        receipt.path,
        receipt.content_kind.as_str(),
        read_file_hashline_header(receipt),
        receipt.line_start,
        receipt.line_end,
        receipt.total_lines,
        receipt
            .next_offset
            .map_or_else(|| "end".to_owned(), |value| value.to_string()),
        receipt.returned_bytes,
        receipt.truncated,
        receipt
            .filter_level_applied
            .map(axon_output_filter::lsp_symbol_filter::LspFilterLevel::as_str)
            .unwrap_or("none")
    );
    if receipt.preview.is_empty() {
        header
    } else {
        format!("{header}\n{}", receipt.preview)
    }
}

fn read_file_hashline_metadata(receipt: &ReadFileToolExecutionReceipt) -> serde_json::Value {
    receipt
        .snapshot_tag
        .as_ref()
        .map_or(serde_json::Value::Null, |tag| {
            serde_json::json!({
                "path": receipt.path,
                "tag": tag,
                "header": format!("[{}#{}]", receipt.path, tag),
            })
        })
}

fn read_file_hashline_header(receipt: &ReadFileToolExecutionReceipt) -> String {
    receipt.snapshot_tag.as_ref().map_or_else(
        || "unavailable".to_owned(),
        |tag| format!("[{}#{}]", receipt.path, tag),
    )
}
