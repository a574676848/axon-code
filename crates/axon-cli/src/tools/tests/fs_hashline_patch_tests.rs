use super::*;
use std::fs;

use crate::tool_orchestrator::ToolExecutionRequest;

#[test]
fn execute_edit_patch_move_counts_one_operation() {
    let workspace = temp_workspace("edit-patch-move-count");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let source = workspace.join("src").join("old.rs");
    let destination = workspace.join("src").join("new.rs");
    fs::write(&source, "pub fn old() {}\n").unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "edit-patch-move-count",
                &serde_json::json!({
                    "input": "*** Begin Patch\n*** Update File: src/old.rs\n*** Move to: src/new.rs\n@@\n-pub fn old() {}\n+pub fn new() {}\n*** End Patch"
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:patch:move".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &context(&workspace),
        None,
    );

    assert_eq!(receipt.status, "completed");
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(destination).unwrap(),
        "pub fn new() {}\n"
    );
    let metadata = receipt.metadata.expect("patch move 应返回 metadata");
    assert_eq!(metadata["operationsApplied"], 1);
    assert_eq!(metadata["files"][0]["operation"], "move");
    assert_eq!(metadata["files"][0]["sourcePath"], "src/old.rs");
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_snapshot_missing_provides_actionable_guidance() {
    let workspace = temp_workspace("edit-hashline-missing-guide");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let file = workspace.join("src").join("demo.rs");
    fs::write(&file, "fn demo() {}\n").unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "edit-missing-snapshot",
                &serde_json::json!({
                    "input": "[src/demo.rs#999999]\nSWAP 1:\n-fn demo() {}\n+fn demo() { println!(); }\n"
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:missing:snapshot".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &context(&workspace),
        None,
    );

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("HASHLINE_SNAPSHOT_MISSING"));
    assert!(receipt.summary.contains("builtin.read_file"));
    assert!(receipt.summary.contains("PathDiff 范式"));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_patch_mismatch_provides_structured_diagnostic() {
    let workspace = temp_workspace("edit-patch-diagnostic");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let file = workspace.join("src").join("calc.rs");
    fs::write(&file, "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n").unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "edit-context-mismatch",
                &serde_json::json!({
                    "input": "*** Begin Patch\n*** Update File: src/calc.rs\n@@ pub fn add_numbers(a: i32) -> i32 {\n-    a + b\n+    a + b + 1\n*** End Patch"
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(2),
            part_id: "tool-part:test:context:mismatch".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(2),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &context(&workspace),
        None,
    );

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("HUNK_CONTEXT_MISMATCH"));
    assert!(receipt.summary.contains("Micro-read Snippet"));
    assert!(
        receipt
            .summary
            .contains("pub fn add(a: i32, b: i32) -> i32 {")
    );
    let _ = fs::remove_dir_all(workspace);
}
