use super::*;
use std::fs;

use crate::tool_orchestrator::ToolExecutionRequest;

fn execute_hashline_edit(
    orchestrator_context: &crate::tool_orchestrator::ToolOrchestratorContext,
    id: &str,
    input: String,
) -> crate::tool_orchestrator::ToolReceipt {
    crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(id, &serde_json::json!({ "input": input }), "builtin.edit"),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: format!("tool-part:test:{id}"),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        orchestrator_context,
        None,
    )
}

#[test]
fn execute_edit_hashline_recovers_unique_same_name_snapshot_path() {
    let workspace = temp_workspace("hashline-path-recovery");
    let original_dir = workspace.join("original");
    let moved_dir = workspace.join("moved");
    fs::create_dir_all(&original_dir).unwrap();
    fs::create_dir_all(&moved_dir).unwrap();
    let original = original_dir.join("lib.rs");
    let moved = moved_dir.join("lib.rs");
    let content = "alpha\nbeta\n";
    fs::write(&original, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(original.canonicalize().unwrap(), content, [1, 2])
        .unwrap();
    fs::rename(&original, &moved).unwrap();
    orchestrator_context
        .workspace_files
        .record_read(moved.canonicalize().unwrap(), content, [1, 2])
        .unwrap();

    let receipt = execute_hashline_edit(
        &orchestrator_context,
        "hashline-path-recovery",
        format!(
            "[original/lib.rs#{}]\nSWAP 2.=2:\n+BETTER\n",
            snapshot.tag()
        ),
    );

    assert_eq!(receipt.status, "completed");
    assert_eq!(fs::read_to_string(&moved).unwrap(), "alpha\nBETTER\n");
    let metadata = receipt.metadata.expect("edit 应返回 metadata");
    assert_eq!(metadata["paths"], serde_json::json!(["moved/lib.rs"]));
    assert!(
        metadata["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning
                .as_str()
                .is_some_and(|text| text.contains("恢复为 moved/lib.rs")))
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_rejects_duplicate_canonical_source_before_write() {
    let workspace = temp_workspace("hashline-canonical-source");
    let source_dir = workspace.join("src");
    fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("lib.rs");
    let content = "one\ntwo\nthree\n";
    fs::write(&source, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(source.canonicalize().unwrap(), content, [1, 2, 3])
        .unwrap();

    let receipt = execute_hashline_edit(
        &orchestrator_context,
        "hashline-canonical-source",
        format!(
            "[src/lib.rs#{}]\nSWAP 1.=1:\n+ONE\n[src/./lib.rs#{}]\nSWAP 3.=3:\n+THREE\n",
            snapshot.tag(),
            snapshot.tag()
        ),
    );

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("指向同一文件"));
    assert_eq!(fs::read_to_string(&source).unwrap(), content);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_does_not_recover_to_outside_workspace() {
    let workspace = temp_workspace("hashline-path-outside");
    fs::create_dir_all(&workspace).unwrap();
    let outside = workspace.with_extension("outside");
    fs::create_dir_all(&outside).unwrap();
    let outside_file = outside.join("lib.rs");
    let content = "alpha\nbeta\n";
    fs::write(&outside_file, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(outside_file.canonicalize().unwrap(), content, [1, 2])
        .unwrap();

    let receipt = execute_hashline_edit(
        &orchestrator_context,
        "hashline-path-outside",
        format!("[missing/lib.rs#{}]\nSWAP 2.=2:\n+BETTER\n", snapshot.tag()),
    );

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("工作区内同名现存路径"));
    assert_eq!(fs::read_to_string(&outside_file).unwrap(), content);
    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(outside);
}

#[test]
fn execute_edit_hashline_recovers_ambiguous_same_name_snapshot_by_lexicographic_path() {
    let workspace = temp_workspace("hashline-path-ambiguous");
    let content = "alpha\nbeta\n";
    let mut paths = Vec::new();
    for name in ["zeta", "alpha", "mid"] {
        let dir = workspace.join(name);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        fs::write(&path, content).unwrap();
        paths.push(path);
    }
    let orchestrator_context = context(&workspace);
    let mut tag = None;
    for path in &paths {
        tag = Some(
            orchestrator_context
                .workspace_files
                .record_read(path.canonicalize().unwrap(), content, [1, 2])
                .unwrap()
                .tag()
                .to_owned(),
        );
    }
    let tag = tag.expect("至少记录一个快照");

    let receipt = execute_hashline_edit(
        &orchestrator_context,
        "hashline-path-ambiguous",
        format!("[missing/lib.rs#{tag}]\nSWAP 2.=2:\n+BETTER\n"),
    );

    // 三个候选内容逐字节相同 ⇒ 语义等价 ⇒ 取路径字典序最小者，其余逐字未变。
    assert_eq!(receipt.status, "completed");
    assert_eq!(
        fs::read_to_string(workspace.join("alpha").join("lib.rs")).unwrap(),
        "alpha\nBETTER\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("mid").join("lib.rs")).unwrap(),
        content
    );
    assert_eq!(
        fs::read_to_string(workspace.join("zeta").join("lib.rs")).unwrap(),
        content
    );
    let metadata = receipt.metadata.expect("edit 应返回 metadata");
    assert_eq!(metadata["paths"], serde_json::json!(["alpha/lib.rs"]));
    assert!(
        metadata["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning
                .as_str()
                .is_some_and(|text| text.contains("恢复为 alpha/lib.rs")))
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_recovers_only_the_unmodified_same_name_twin() {
    let workspace = temp_workspace("hashline-path-unmodified-twin");
    let content = "alpha\nbeta\n";
    let first_dir = workspace.join("first");
    let second_dir = workspace.join("second");
    fs::create_dir_all(&first_dir).unwrap();
    fs::create_dir_all(&second_dir).unwrap();
    let first = first_dir.join("lib.rs");
    let second = second_dir.join("lib.rs");
    fs::write(&first, content).unwrap();
    fs::write(&second, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(first.canonicalize().unwrap(), content, [1, 2])
        .unwrap();
    orchestrator_context
        .workspace_files
        .record_read(second.canonicalize().unwrap(), content, [1, 2])
        .unwrap();
    // 读取后 first 被外部改动：它已不再是模型看到的那个版本，必须排除。
    // 注意 first 的路径字典序**小于** second：若实现只是「取字典序最小」而非
    // 先按内容筛选，本用例会去改 first，从而暴露该缺陷。
    fs::write(&first, "alpha\nEXTERNAL\n").unwrap();

    let receipt = execute_hashline_edit(
        &orchestrator_context,
        "hashline-path-unmodified-twin",
        format!("[missing/lib.rs#{}]\nSWAP 2.=2:\n+BETTER\n", snapshot.tag()),
    );

    assert_eq!(receipt.status, "completed");
    assert_eq!(fs::read_to_string(&second).unwrap(), "alpha\nBETTER\n");
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\nEXTERNAL\n");
    let metadata = receipt.metadata.expect("edit 应返回 metadata");
    assert_eq!(metadata["paths"], serde_json::json!(["second/lib.rs"]));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_hashline_rejects_recovery_when_every_same_name_candidate_is_stale() {
    let workspace = temp_workspace("hashline-path-all-stale");
    let content = "alpha\nbeta\n";
    let first_dir = workspace.join("first");
    let second_dir = workspace.join("second");
    fs::create_dir_all(&first_dir).unwrap();
    fs::create_dir_all(&second_dir).unwrap();
    let first = first_dir.join("lib.rs");
    let second = second_dir.join("lib.rs");
    fs::write(&first, content).unwrap();
    fs::write(&second, content).unwrap();
    let orchestrator_context = context(&workspace);
    let snapshot = orchestrator_context
        .workspace_files
        .record_read(first.canonicalize().unwrap(), content, [1, 2])
        .unwrap();
    orchestrator_context
        .workspace_files
        .record_read(second.canonicalize().unwrap(), content, [1, 2])
        .unwrap();
    // 两个同名候选都被外部改动：已无任何候选代表模型看到的版本，必须保守拒绝。
    fs::write(&first, "alpha\nEXTERNAL\n").unwrap();
    fs::write(&second, "alpha\nALSO\n").unwrap();

    let receipt = execute_hashline_edit(
        &orchestrator_context,
        "hashline-path-all-stale",
        format!("[missing/lib.rs#{}]\nSWAP 2.=2:\n+BETTER\n", snapshot.tag()),
    );

    assert_eq!(receipt.status, "failed");
    assert!(receipt.summary.contains("没有唯一的工作区内同名现存路径"));
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\nEXTERNAL\n");
    assert_eq!(fs::read_to_string(&second).unwrap(), "alpha\nALSO\n");
    let _ = fs::remove_dir_all(workspace);
}
