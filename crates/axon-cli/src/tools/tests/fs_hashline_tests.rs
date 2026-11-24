use super::*;
use std::fs;

use crate::tool_orchestrator::ToolExecutionRequest;

#[test]
fn execute_edit_hashline_exposes_multi_file_structured_diff() {
    let workspace = temp_workspace("hashline-edit-report");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let lib_path = workspace.join("src").join("lib.rs");
    let main_path = workspace.join("src").join("main.rs");
    let lib_content = "alpha\nbeta\ngamma\n";
    let main_content = "one\ntwo\n";
    fs::write(&lib_path, lib_content).unwrap();
    fs::write(&main_path, main_content).unwrap();
    let orchestrator_context = context(&workspace);
    let lib_snapshot = orchestrator_context
        .workspace_files
        .record_read(lib_path.canonicalize().unwrap(), lib_content, [1, 2, 3])
        .unwrap();
    let main_snapshot = orchestrator_context
        .workspace_files
        .record_read(main_path.canonicalize().unwrap(), main_content, [1, 2])
        .unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "hashline-edit-report",
                &serde_json::json!({
                    "input": format!(
                        "[src/lib.rs#{}]\nSWAP 2.=2:\n+BETTER\n[src/main.rs#{}]\nINS.POST 2:\n+three\n",
                        lib_snapshot.tag(),
                        main_snapshot.tag()
                    )
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:hashline:request".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &orchestrator_context,
        None,
    );

    assert_eq!(receipt.status, "completed");
    assert_eq!(
        fs::read_to_string(&lib_path).unwrap(),
        "alpha\nBETTER\ngamma\n"
    );
    assert_eq!(fs::read_to_string(&main_path).unwrap(), "one\ntwo\nthree\n");
    let metadata = receipt.metadata.expect("edit 应返回 metadata");
    assert_eq!(metadata["kind"], "edit");
    assert_eq!(metadata["strategy"], "hashline");
    assert_eq!(metadata["fileCount"], 2);
    assert_eq!(
        metadata["paths"],
        serde_json::json!(["src/lib.rs", "src/main.rs"])
    );
    assert_eq!(metadata["files"].as_array().unwrap().len(), 2);
    assert!(
        metadata["files"][0]["diff"]
            .as_str()
            .expect("diff")
            .contains("+BETTER")
    );
    assert_eq!(metadata["files"][0]["hashline"]["path"], "src/lib.rs");
    assert_eq!(
        metadata["files"][0]["hashline"]["tag"],
        axon_agent_runtime::hashline_snapshot_tag("alpha\nBETTER\ngamma\n")
    );
    assert_eq!(
        metadata["files"][0]["hashline"]["header"],
        format!(
            "[src/lib.rs#{}]",
            metadata["files"][0]["hashline"]["tag"].as_str().unwrap()
        )
    );
    assert!(
        metadata["files"][1]["diff"]
            .as_str()
            .expect("diff")
            .contains("+three")
    );
    assert_eq!(metadata["files"][1]["hashline"]["path"], "src/main.rs");
    assert_eq!(
        metadata["files"][1]["hashline"]["tag"],
        axon_agent_runtime::hashline_snapshot_tag("one\ntwo\nthree\n")
    );
    assert_eq!(
        metadata["files"][1]["hashline"]["header"],
        format!(
            "[src/main.rs#{}]",
            metadata["files"][1]["hashline"]["tag"].as_str().unwrap()
        )
    );
    let lsp_files = metadata["lsp"]["files"]
        .as_array()
        .expect("edit 写后必须返回 LSP 调度文件");
    assert!(
        lsp_files
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|path| path.replace('\\', "/") == "src/lib.rs")
    );
    assert!(
        lsp_files
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|path| path.replace('\\', "/") == "src/main.rs")
    );
    let lib_tag = metadata["files"][0]["hashline"]["tag"].as_str().unwrap();
    assert!(matches!(
        orchestrator_context
            .workspace_files
            .lookup(&lib_path.canonicalize().unwrap(), lib_tag,),
        axon_agent_runtime::WorkspaceFileSnapshotLookup::Found(_)
    ));
    let main_tag = metadata["files"][1]["hashline"]["tag"].as_str().unwrap();
    assert!(matches!(
        orchestrator_context
            .workspace_files
            .lookup(&main_path.canonicalize().unwrap(), main_tag,),
        axon_agent_runtime::WorkspaceFileSnapshotLookup::Found(_)
    ));
    let _ = fs::remove_dir_all(workspace);
}
#[test]
fn execute_edit_routes_complete_patch_to_apply_patch_strategy() {
    let workspace = temp_workspace("hashline-apply-patch-fallback");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let path = workspace.join("src").join("lib.rs");
    fs::write(&path, "pub fn old() {}\n").unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "hashline-apply-patch-fallback",
                &serde_json::json!({
                    "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-pub fn old() {}\n+pub fn new() {}\n*** End Patch",
                    "summary": "传统 patch 兜底"
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:hashline:request".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &context(&workspace),
        None,
    );

    assert_eq!(receipt.status, "completed");
    assert_eq!(fs::read_to_string(path).unwrap(), "pub fn new() {}\n");
    let metadata = receipt.metadata.expect("edit 应返回 metadata");
    assert_eq!(metadata["kind"], "edit");
    assert_eq!(metadata["strategy"], "apply_patch");
    assert_eq!(metadata["files"][0]["path"], "src/lib.rs");
    assert_eq!(metadata["files"][0]["operation"], "update");
    assert_eq!(metadata["files"][0]["hashline"]["path"], "src/lib.rs");
    assert_eq!(
        metadata["files"][0]["hashline"]["tag"],
        axon_agent_runtime::hashline_snapshot_tag("pub fn new() {}\n")
    );
    let _ = fs::remove_dir_all(workspace);
}
#[test]
fn execute_edit_pathdiff_supports_add_delete_and_move() {
    let workspace = temp_workspace("pathdiff-file-operations");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let source = workspace.join("src").join("lib.rs");
    let deleted = workspace.join("old.txt");
    fs::write(&source, "pub fn old() {}\n").unwrap();
    fs::write(&deleted, "remove me\n").unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "pathdiff-file-operations",
                &serde_json::json!({
                    "input": "*** Begin Patch\n*** Add File: nested/new.txt\n+created\n*** Delete File: old.txt\n*** Update File: src/lib.rs\n*** Move to: src/moved.rs\n@@\n-pub fn old() {}\n+pub fn moved() {}\n*** End Patch"
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:pathdiff:request".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &context(&workspace),
        None,
    );

    assert_eq!(receipt.status, "completed");
    assert_eq!(
        fs::read_to_string(workspace.join("nested").join("new.txt")).unwrap(),
        "created\n"
    );
    assert!(!deleted.exists());
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(workspace.join("src").join("moved.rs")).unwrap(),
        "pub fn moved() {}\n"
    );
    let metadata = receipt.metadata.expect("PathDiff 应返回 metadata");
    assert_eq!(metadata["strategy"], "apply_patch");
    assert_eq!(metadata["operationsApplied"], 3);
    let operations = metadata["files"]
        .as_array()
        .expect("PathDiff 应返回文件操作");
    assert_eq!(operations[0]["operation"], "add");
    assert_eq!(operations[1]["operation"], "delete");
    assert_eq!(operations[2]["operation"], "move");
    assert_eq!(operations[1]["diff"], "remove me\n");
    assert_eq!(operations[2]["sourcePath"], "src/lib.rs");
    assert_eq!(operations[2]["seenLines"], serde_json::json!([1]));
    assert_eq!(
        metadata["paths"],
        serde_json::json!(["nested/new.txt", "old.txt", "src/lib.rs", "src/moved.rs"])
    );
    let lsp_files = metadata["lsp"]["files"]
        .as_array()
        .expect("PathDiff 写后必须返回 LSP 调度文件");
    assert!(
        lsp_files
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|path| path.replace('\\', "/") == "src/lib.rs")
    );
    assert!(
        lsp_files
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|path| path.replace('\\', "/") == "src/moved.rs")
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_preserves_existing_text_format() {
    let workspace = temp_workspace("hashline-text-format");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let path = workspace.join("src").join("lib.rs");
    let content = "\u{feff}one\r\ntwo\r\n";
    fs::write(&path, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(path.canonicalize().unwrap(), content, [1, 2])
        .unwrap();
    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "hashline-text-format",
                &serde_json::json!({
                    "input": format!("[src/lib.rs#{}]\nSWAP 2.=2:\n+TWO", snapshot.tag())
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:hashline:text-format".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &orchestrator_context,
        None,
    );

    let expected = "\u{feff}one\r\nTWO\r\n";
    assert_eq!(receipt.status, "completed");
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    let metadata = receipt.metadata.unwrap();
    assert_eq!(
        metadata["files"][0]["hashline"]["tag"],
        axon_agent_runtime::hashline_snapshot_tag(expected)
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_rejects_identical_noop_without_attempt_budget() {
    let workspace = temp_workspace("hashline-noop-guard");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let path = workspace.join("src").join("lib.rs");
    let content = "alpha\nbeta\ngamma\n";
    fs::write(&path, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(path.canonicalize().unwrap(), content, [2])
        .unwrap();
    let input = format!("[src/lib.rs#{}]\nSWAP 2.=2:\n+beta\n", snapshot.tag());
    let request = ToolExecutionRequest {
        tool_call: tool_call(
            "hashline-noop-guard",
            &serde_json::json!({ "input": input }),
            "builtin.edit",
        ),
        tool_call_id: axon_domain::ToolCallId(1),
        part_id: "tool-part:test:hashline:request".to_owned(),
        execution_request_id: axon_domain::ExecutionRequestId(1),
        kind: crate::tool_orchestrator::ToolKind::Edit,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        execution_permission: axon_domain::ToolExecutionPermission::Allowed,
    };
    let first =
        crate::tool_orchestrator_hashline::execute_edit(&request, &orchestrator_context, None);
    assert_eq!(first.status, "failed");
    assert!(first.summary.contains("未产生内容变化"));
    assert_eq!(fs::read_to_string(path).unwrap(), content);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_removes_file_and_invalidates_snapshot() {
    let workspace = temp_workspace("hashline-rem");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let path = workspace.join("src").join("old.rs");
    let content = "pub fn old() {}\n";
    fs::write(&path, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(path.canonicalize().unwrap(), content, [1])
        .unwrap();
    let request = ToolExecutionRequest {
        tool_call: tool_call(
            "hashline-rem",
            &serde_json::json!({
                "input": format!("[src/old.rs#{}]\nREM", snapshot.tag())
            }),
            "builtin.edit",
        ),
        tool_call_id: axon_domain::ToolCallId(1),
        part_id: "tool-part:test:hashline:rem".to_owned(),
        execution_request_id: axon_domain::ExecutionRequestId(1),
        kind: crate::tool_orchestrator::ToolKind::Edit,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        execution_permission: axon_domain::ToolExecutionPermission::Allowed,
    };

    let receipt =
        crate::tool_orchestrator_hashline::execute_edit(&request, &orchestrator_context, None);

    assert_eq!(receipt.status, "completed");
    assert!(!path.exists());
    let metadata = receipt.metadata.expect("REM 应返回 edit metadata");
    assert_eq!(metadata["strategy"], "hashline");
    assert_eq!(metadata["operationsApplied"], 1);
    assert_eq!(metadata["files"][0]["operation"], "delete");
    assert!(matches!(
        orchestrator_context
            .workspace_files
            .lookup(&path, snapshot.tag()),
        axon_agent_runtime::WorkspaceFileSnapshotLookup::Missing
    ));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_applies_changes_then_moves_file() {
    let workspace = temp_workspace("hashline-mv");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let source = workspace.join("src").join("old.rs");
    let destination = workspace.join("src").join("new.rs");
    let content = "one\ntwo\nthree\n";
    fs::write(&source, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(source.canonicalize().unwrap(), content, [1, 2, 3])
        .unwrap();
    let request = ToolExecutionRequest {
        tool_call: tool_call(
            "hashline-mv",
            &serde_json::json!({
                "input": format!(
                    "[src/old.rs#{}]\nSWAP 2.=2:\n+TWO\nMV src/new.rs",
                    snapshot.tag()
                )
            }),
            "builtin.edit",
        ),
        tool_call_id: axon_domain::ToolCallId(1),
        part_id: "tool-part:test:hashline:mv".to_owned(),
        execution_request_id: axon_domain::ExecutionRequestId(1),
        kind: crate::tool_orchestrator::ToolKind::Edit,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        execution_permission: axon_domain::ToolExecutionPermission::Allowed,
    };

    let receipt =
        crate::tool_orchestrator_hashline::execute_edit(&request, &orchestrator_context, None);

    assert_eq!(receipt.status, "completed");
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "one\nTWO\nthree\n"
    );
    let metadata = receipt.metadata.expect("MV 应返回 edit metadata");
    assert_eq!(metadata["files"][0]["path"], "src/new.rs");
    assert_eq!(metadata["files"][0]["sourcePath"], "src/old.rs");
    assert_eq!(metadata["files"][0]["operation"], "move");
    let destination_tag = metadata["files"][0]["hashline"]["tag"]
        .as_str()
        .expect("移动目标应生成 Hashline 快照");
    assert!(matches!(
        orchestrator_context
            .workspace_files
            .lookup(&destination.canonicalize().unwrap(), destination_tag),
        axon_agent_runtime::WorkspaceFileSnapshotLookup::Found(_)
    ));
    assert!(matches!(
        orchestrator_context
            .workspace_files
            .lookup(&source, snapshot.tag()),
        axon_agent_runtime::WorkspaceFileSnapshotLookup::Missing
    ));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_rejects_move_conflict_before_any_write() {
    let workspace = temp_workspace("hashline-mv-conflict");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let first = workspace.join("src").join("first.rs");
    let source = workspace.join("src").join("old.rs");
    let destination = workspace.join("src").join("new.rs");
    fs::write(&first, "first\n").unwrap();
    fs::write(&source, "old\n").unwrap();
    fs::write(&destination, "occupied\n").unwrap();
    let orchestrator_context = context(&workspace);
    let first_snapshot = orchestrator_context
        .workspace_files
        .record_read(first.canonicalize().unwrap(), "first\n", [1])
        .unwrap();
    let source_snapshot = orchestrator_context
        .workspace_files
        .record_read(source.canonicalize().unwrap(), "old\n", [1])
        .unwrap();
    let request = ToolExecutionRequest {
        tool_call: tool_call(
            "hashline-mv-conflict",
            &serde_json::json!({
                "input": format!(
                    "[src/first.rs#{}]\nSWAP 1.=1:\n+changed\n[src/old.rs#{}]\nMV src/new.rs",
                    first_snapshot.tag(),
                    source_snapshot.tag()
                )
            }),
            "builtin.edit",
        ),
        tool_call_id: axon_domain::ToolCallId(1),
        part_id: "tool-part:test:hashline:mv-conflict".to_owned(),
        execution_request_id: axon_domain::ExecutionRequestId(1),
        kind: crate::tool_orchestrator::ToolKind::Edit,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        execution_permission: axon_domain::ToolExecutionPermission::Allowed,
    };

    let receipt =
        crate::tool_orchestrator_hashline::execute_edit(&request, &orchestrator_context, None);

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("MV 目标已存在"));
    assert_eq!(fs::read_to_string(first).unwrap(), "first\n");
    assert_eq!(fs::read_to_string(source).unwrap(), "old\n");
    assert_eq!(fs::read_to_string(destination).unwrap(), "occupied\n");
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_allows_parent_move_and_applies_all_operations() {
    let workspace = temp_workspace("hashline-unsafe-move");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let first = workspace.join("src").join("first.rs");
    let source = workspace.join("src").join("old.rs");
    fs::write(&first, "first\n").unwrap();
    fs::write(&source, "old\n").unwrap();
    let escaped_name = format!(
        "{}.escaped",
        workspace
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
    );
    let escaped = workspace.parent().unwrap().join(&escaped_name);
    let _ = fs::remove_file(&escaped);
    let orchestrator_context = context(&workspace);
    let first_snapshot = orchestrator_context
        .workspace_files
        .record_read(first.canonicalize().unwrap(), "first\n", [1])
        .unwrap();
    let source_snapshot = orchestrator_context
        .workspace_files
        .record_read(source.canonicalize().unwrap(), "old\n", [1])
        .unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "hashline-unsafe-move",
                &serde_json::json!({
                    "input": format!(
                        "[src/first.rs#{}]\nSWAP 1.=1:\n+changed\n[src/old.rs#{}]\nMV ../{escaped_name}",
                        first_snapshot.tag(),
                        source_snapshot.tag()
                    )
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:hashline:unsafe-move".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &orchestrator_context,
        None,
    );

    assert_eq!(receipt.status, "completed");
    assert_eq!(fs::read_to_string(first).unwrap(), "changed\n");
    assert!(!source.exists());
    assert_eq!(fs::read_to_string(&escaped).unwrap(), "old\n");
    let _ = fs::remove_file(&escaped);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_rejects_duplicate_move_destination_before_any_write() {
    let workspace = temp_workspace("hashline-duplicate-mv-target");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let first = workspace.join("src").join("first.rs");
    let second = workspace.join("src").join("second.rs");
    fs::write(&first, "first\n").unwrap();
    fs::write(&second, "second\n").unwrap();
    let orchestrator_context = context(&workspace);
    let first_snapshot = orchestrator_context
        .workspace_files
        .record_read(first.canonicalize().unwrap(), "first\n", [1])
        .unwrap();
    let second_snapshot = orchestrator_context
        .workspace_files
        .record_read(second.canonicalize().unwrap(), "second\n", [1])
        .unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "hashline-duplicate-mv-target",
                &serde_json::json!({
                    "input": format!(
                        "[src/first.rs#{}]\nMV src/shared.rs\n[src/second.rs#{}]\nMV src/shared.rs\n",
                        first_snapshot.tag(),
                        second_snapshot.tag()
                    )
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:hashline-duplicate-mv-target:request".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &orchestrator_context,
        None,
    );

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("同一目标路径"));
    assert_eq!(fs::read_to_string(first).unwrap(), "first\n");
    assert_eq!(fs::read_to_string(second).unwrap(), "second\n");
    assert!(!workspace.join("src").join("shared.rs").exists());
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_rejects_move_destination_used_as_batch_source() {
    let workspace = temp_workspace("hashline-mv-target-source");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let first = workspace.join("src").join("first.rs");
    let second = workspace.join("src").join("second.rs");
    fs::write(&first, "first\n").unwrap();
    fs::write(&second, "second\n").unwrap();
    let orchestrator_context = context(&workspace);
    let first_snapshot = orchestrator_context
        .workspace_files
        .record_read(first.canonicalize().unwrap(), "first\n", [1])
        .unwrap();
    let second_snapshot = orchestrator_context
        .workspace_files
        .record_read(second.canonicalize().unwrap(), "second\n", [1])
        .unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "hashline-mv-target-source",
                &serde_json::json!({
                    "input": format!(
                        "[src/first.rs#{}]\nMV src/second.rs\n[src/second.rs#{}]\nSWAP 1.=1:\n+changed\n",
                        first_snapshot.tag(),
                        second_snapshot.tag()
                    )
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:hashline-mv-target-source:request".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &orchestrator_context,
        None,
    );

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("同批次源路径"));
    assert_eq!(fs::read_to_string(first).unwrap(), "first\n");
    assert_eq!(fs::read_to_string(second).unwrap(), "second\n");
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_emits_file_progress() {
    let workspace = temp_workspace("hashline-edit-progress");
    fs::create_dir_all(workspace.join("src")).unwrap();
    let path = workspace.join("src").join("lib.rs");
    let content = "alpha\nbeta\ngamma\n";
    fs::write(&path, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(path.canonicalize().unwrap(), content, [2])
        .unwrap();
    let request = ToolExecutionRequest {
        tool_call: tool_call(
            "hashline-edit-progress",
            &serde_json::json!({
                "input": format!("[src/lib.rs#{}]\nSWAP 2.=2:\n+BETTER\n", snapshot.tag())
            }),
            "builtin.edit",
        ),
        tool_call_id: axon_domain::ToolCallId(1),
        part_id: "tool-part:test:hashline:request".to_owned(),
        execution_request_id: axon_domain::ExecutionRequestId(1),
        kind: crate::tool_orchestrator::ToolKind::Edit,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        execution_permission: axon_domain::ToolExecutionPermission::Allowed,
    };
    let mut events = Vec::new();
    let mut observer = |event| events.push(event);

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &request,
        &orchestrator_context,
        Some(&mut observer),
    );

    assert_eq!(receipt.status, "completed");
    assert_eq!(events.len(), 2);
    for (index, event) in events.iter().enumerate() {
        let crate::tool_orchestrator::ToolRuntimeEvent::Progress(progress) = event else {
            panic!("edit 应发出文件进度事件");
        };
        assert_eq!(progress.phase, "applying");
        assert_eq!(progress.current_item.as_deref(), Some("src/lib.rs"));
        assert_eq!(progress.items_completed, Some(index as u64));
        assert_eq!(progress.items_total, Some(1));
    }
    let _ = fs::remove_dir_all(workspace);
}
