//! edit 执行回执投影。

use crate::{
    AgentRunToolReceiptProjection, HASHLINE_REPAIR_WARNING_PREFIX, ProviderToolCall,
    agent_run_model_tools::agent_run_model_tool_diagnostics::{
        apply_patch_diagnostics_clusters, apply_patch_diagnostics_summary,
        post_apply_diagnostics_errors,
    },
};

const EDIT_NEXT_ACTION_REUSE_SNAPSHOT: &str =
    "后续同一文件的小范围编辑优先复用 files[].hashline.header";
const EDIT_NEXT_ACTION_RUN_CHECKS: &str = "运行受影响模块的目标检查或测试";
/// 存在自动修复时追加的复核指引。
///
/// 修复会改变模型提交的原文；只写进 warnings 容易被忽略，
/// 因此在 nextActions 中显式要求复核 diff：修复不得静默。
const EDIT_NEXT_ACTION_REVIEW_AUTO_REPAIR: &str =
    "本次编辑包含自动修复（见 warnings 中 [修复] 前缀项），请复核 files[].diff 确认改动符合原意";

/// Facts from a completed local edit execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunEditToolReceiptInput {
    /// Provider tool call.
    pub tool_call: ProviderToolCall,
    /// 实际采用的内部编辑策略。
    pub strategy: String,
    /// Workspace-relative source and target paths.
    pub paths: Vec<String>,
    /// Provider/user-visible summary.
    pub summary: String,
    /// Structured file changes for TUI transcript rendering.
    pub files: serde_json::Value,
    /// Applied file operation count.
    pub operations_applied: usize,
    /// Edit warnings.
    pub warnings: Vec<String>,
    /// Unified post-write LSP refresh insight.
    pub lsp: serde_json::Value,
}

/// Project a completed edit execution into the provider-visible tool receipt contract.
#[must_use]
pub fn agent_run_edit_tool_receipt(
    input: AgentRunEditToolReceiptInput,
) -> AgentRunToolReceiptProjection {
    let file_count = input.files.as_array().map_or(0, std::vec::Vec::len);
    let diagnostics_summary = apply_patch_diagnostics_summary(&input.lsp);
    let diagnostics_clusters = apply_patch_diagnostics_clusters(&diagnostics_summary);
    let post_apply_diagnostics_errors = post_apply_diagnostics_errors(&diagnostics_clusters);
    let next_actions = edit_next_actions(
        &input.files,
        &post_apply_diagnostics_errors,
        &input.warnings,
    );
    let metadata = serde_json::json!({
        "kind": "edit",
        "strategy": input.strategy,
        "paths": input.paths,
        "fileCount": file_count,
        "summary": input.summary,
        "files": input.files,
        "operationsApplied": input.operations_applied,
        "warnings": input.warnings,
        "diagnosticsSummary": diagnostics_summary,
        "diagnosticsClusters": diagnostics_clusters,
        "postApplyDiagnosticsErrors": post_apply_diagnostics_errors,
        "lsp": input.lsp,
        "nextActions": next_actions,
    });
    let summary = agent_run_edit_tool_receipt_summary(
        metadata["fileCount"].as_u64().unwrap_or_default(),
        input.operations_applied,
        metadata["warnings"]
            .as_array()
            .map_or(0, std::vec::Vec::len),
        metadata["summary"].as_str().unwrap_or_default(),
    );
    AgentRunToolReceiptProjection {
        provider_call_id: input.tool_call.id,
        tool_name: input.tool_call.name,
        status: "completed".to_owned(),
        summary,
        metadata: Some(metadata),
        output_ref: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: input.operations_applied > 0,
    }
}

/// Project edit execution facts into a bounded provider-visible summary.
#[must_use]
pub fn agent_run_edit_tool_receipt_summary(
    files: u64,
    operations_applied: usize,
    warnings: usize,
    summary: &str,
) -> String {
    format!(
        "edit files={files} operations_applied={operations_applied} warnings={warnings} summary={summary}"
    )
}

fn edit_next_actions(
    files: &serde_json::Value,
    post_apply_diagnostics_errors: &serde_json::Value,
    warnings: &[String],
) -> serde_json::Value {
    let snapshot_headers = files
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| {
            file.get("hashline")
                .and_then(|snapshot| snapshot.get("header"))
        })
        .filter_map(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let has_diagnostics_errors = post_apply_diagnostics_errors
        .as_array()
        .is_some_and(|errors| !errors.is_empty());
    let mut guidance = vec![EDIT_NEXT_ACTION_REUSE_SNAPSHOT, EDIT_NEXT_ACTION_RUN_CHECKS];
    if has_auto_repair(warnings) {
        guidance.push(EDIT_NEXT_ACTION_REVIEW_AUTO_REPAIR);
    }

    serde_json::json!({
        "snapshotHeaders": snapshot_headers,
        "verify": {
            "reviewFilesDiff": true,
            "fixPostApplyDiagnostics": has_diagnostics_errors,
            "runTargetedChecks": true,
        },
        "guidance": guidance,
    })
}

/// 本次编辑是否发生过自动修复。
///
/// 以前缀识别而非匹配整句：修复 warning 的正文随修复种类变化，
/// 只有前缀是两侧共用的稳定契约（`HASHLINE_REPAIR_WARNING_PREFIX`）。
fn has_auto_repair(warnings: &[String]) -> bool {
    warnings
        .iter()
        .any(|warning| warning.starts_with(HASHLINE_REPAIR_WARNING_PREFIX))
}
