use super::*;
use axon_domain::{CellId, SessionId, ToolCallInput};
use serde_json::Value;
use std::fs;
use std::time::Duration;

use crate::tool_orchestrator::ToolExecutionRequest;

const TEST_FREEFORM_PATCH: &str =
    "*** Begin Patch\n*** Add File: src/new.rs\n+fn main() {}\n*** End Patch";

/// 相对工作区前缀。
///
/// 本测试断言的是**相对**工作目录的读取路径，工作区因此必须落在当前工作目录之下，不能用系统
/// 临时目录；改为自建带唯一后缀的目录，并由 [`RelativeWorkspaceGuard`] 在测试结束（含 panic
/// 展开）时整体删除，不在 crate 目录留下 `target/` 之类的构建残留。
const RELATIVE_WORKSPACE_PREFIX: &str = ".axon-relative-workspace-";

/// 相对工作区清理守卫：离开作用域即删除整个工作区。
struct RelativeWorkspaceGuard(std::path::PathBuf);

impl Drop for RelativeWorkspaceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[path = "fs_hashline_patch_tests.rs"]
mod fs_hashline_patch_tests;
#[path = "fs_hashline_path_tests.rs"]
mod fs_hashline_path_tests;
#[path = "fs_hashline_tests.rs"]
mod fs_hashline_tests;

#[test]
fn execute_read_file_schedules_non_blocking_lsp_touch_with_relative_cwd() {
    let current_dir = std::env::current_dir().unwrap();
    let workspace = current_dir.join(format!(
        "{RELATIVE_WORKSPACE_PREFIX}{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos()
    ));
    let _workspace_guard = RelativeWorkspaceGuard(workspace.clone());
    let relative_workspace = workspace.strip_prefix(&current_dir).unwrap().to_path_buf();
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(workspace.join("src").join("main.rs"), "fn main() {}\n").unwrap();

    let receipt = execute_read_file(
        &tool_call(
            "read-file-background",
            &serde_json::json!({"path":"src/main.rs"}),
            "builtin.read_file",
        ),
        &context(&relative_workspace),
    );
    assert_eq!(receipt.status, "completed");
    assert!(receipt.summary.ends_with("\n1:fn main() {}"));
    let metadata = receipt.metadata.expect("read_file 应返回 metadata");
    assert!(metadata.get("preview").is_none());
    assert!(metadata.get("lspReceipt").is_none());
    assert!(metadata["lspBackground"]["status"].is_string());
    assert!(metadata["lspBackground"]["files"].is_array());
    assert!(metadata["lspBackground"]["coalesced"].is_number());
    assert!(metadata["lspBackground"]["evicted"].is_number());
    let scheduled_file = metadata["lspBackground"]["files"][0]
        .as_str()
        .expect("支持 LSP 的目标文件应进入后台调度");
    assert!(scheduled_file.replace('\\', "/").ends_with("/src/main.rs"));
    assert_eq!(metadata["page"]["lineStart"], 1);
    assert_eq!(metadata["page"]["totalLines"], 1);
    assert_eq!(metadata["hashline"]["path"], "src/main.rs");
    assert_eq!(metadata["seenLines"], serde_json::json!([1]));
}

#[test]
fn execute_edit_patch_exposes_file_level_reports() {
    let workspace = temp_workspace("apply-patch-report");
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: axon_agent_runtime::ProviderToolCall::new(
                "apply-patch-report",
                "builtin.edit",
                ToolCallInput::text(TEST_FREEFORM_PATCH),
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:fs:request".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &context(&workspace),
        None,
    );
    assert_eq!(receipt.status, "completed");
    let metadata = receipt.metadata.expect("edit 应返回 metadata");
    assert_eq!(metadata["kind"], "edit");
    assert_eq!(metadata["strategy"], "apply_patch");
    assert_eq!(metadata["files"].as_array().map(Vec::len), Some(1));
    assert!(metadata.get("truncated").is_none());
    if let Some(first_file) = metadata["files"].as_array().and_then(|items| items.first()) {
        assert!(first_file.get("diagnostics").is_none());
        assert_eq!(first_file["path"], "src/new.rs");
    }
    let first_document = metadata["lsp"]["files"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(serde_json::Value::as_str)
        .expect("写入后应调度变更文档");
    assert_eq!(first_document.replace('\\', "/"), "src/new.rs");
    assert!(metadata["lsp"]["status"].is_string());
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_patch_exposes_structured_file_diff() {
    let workspace = temp_workspace("apply-diff-report");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(
        workspace.join("src").join("lib.rs"),
        "fn keep_before() {}\npub fn old() {}\nfn keep_after() {}\n",
    )
    .unwrap();
    let orchestrator_context = context(&workspace);

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "edit-patch-report",
                &serde_json::json!({
                    "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n fn keep_before() {}\n-pub fn old() {}\n+pub fn new() {}\n fn keep_after() {}\n*** End Patch"
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:fs:request".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &orchestrator_context,
        None,
    );

    assert_eq!(receipt.status, "completed");
    let metadata = receipt.metadata.expect("edit 应返回 metadata");
    assert_eq!(metadata["files"][0]["path"], "src/lib.rs");
    assert_eq!(metadata["files"][0]["operation"], "update");
    let diff = metadata["files"][0]["diff"].as_str().expect("diff");
    assert!(diff.contains("@@"));
    assert!(diff.contains("-pub fn old() {}"));
    assert!(diff.contains("+pub fn new() {}"));
    assert_eq!(metadata["files"][0]["hashline"]["path"], "src/lib.rs");
    assert_eq!(
        metadata["files"][0]["hashline"]["tag"],
        axon_agent_runtime::hashline_snapshot_tag(
            "fn keep_before() {}\npub fn new() {}\nfn keep_after() {}\n"
        )
    );
    assert_eq!(
        metadata["files"][0]["hashline"]["header"],
        format!(
            "[src/lib.rs#{}]",
            metadata["files"][0]["hashline"]["tag"].as_str().unwrap()
        )
    );
    assert!(metadata["lsp"]["files"].is_array());
    let path = workspace.join("src").join("lib.rs").canonicalize().unwrap();
    let tag = metadata["files"][0]["hashline"]["tag"].as_str().unwrap();
    assert!(matches!(
        orchestrator_context.workspace_files.lookup(&path, tag),
        axon_agent_runtime::WorkspaceFileSnapshotLookup::Found(snapshot)
            if snapshot.seen_lines()
                == &std::collections::BTreeSet::from([1, 2, 3])
    ));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_patch_inherits_existing_text_format() {
    let workspace = temp_workspace("apply-diff-format");
    let path = workspace.join("format.txt");
    fs::write(&path, "\u{feff}before\r\nold\r\n").unwrap();

    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "edit-patch-format",
                &serde_json::json!({
                    "input": "*** Begin Patch\n*** Update File: format.txt\n@@\n-old\n+new\n*** End Patch"
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            part_id: "part-apply-diff-format".to_owned(),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &context(&workspace),
        None,
    );

    assert_eq!(receipt.status, "completed");
    assert_eq!(
        fs::read(&path).unwrap(),
        "\u{feff}before\r\nnew\r\n".as_bytes()
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_edit_patch_exposes_post_write_hashline_snapshots() {
    let workspace = temp_workspace("apply-patch-hashline");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(workspace.join("src").join("lib.rs"), "pub fn old() {}\n").unwrap();
    fs::write(workspace.join("src").join("gone.rs"), "pub fn gone() {}\n").unwrap();
    let orchestrator_context = context(&workspace);
    let gone_snapshot = orchestrator_context
        .workspace_files
        .record_read(
            workspace
                .join("src")
                .join("gone.rs")
                .canonicalize()
                .unwrap(),
            "pub fn gone() {}\n",
            [1],
        )
        .unwrap();
    let receipt = crate::tool_orchestrator_hashline::execute_edit(
        &ToolExecutionRequest {
            tool_call: tool_call(
                "apply-patch-hashline",
                &serde_json::json!({
                    "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-pub fn old() {}\n+pub fn edited() {}\n*** Add File: src/new.rs\n+pub fn new() {}\n*** Delete File: src/gone.rs\n*** End Patch"
                }),
                "builtin.edit",
            ),
            tool_call_id: axon_domain::ToolCallId(1),
            part_id: "tool-part:test:fs:request".to_owned(),
            execution_request_id: axon_domain::ExecutionRequestId(1),
            kind: crate::tool_orchestrator::ToolKind::Edit,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: axon_domain::ToolExecutionPermission::Allowed,
        },
        &orchestrator_context,
        None,
    );

    assert_eq!(receipt.status, "completed");
    let metadata = receipt.metadata.expect("edit 应返回 metadata");
    let files = metadata["files"].as_array().expect("files");
    let updated = files
        .iter()
        .find(|file| file["path"] == "src/lib.rs")
        .expect("updated file");
    let added = files
        .iter()
        .find(|file| file["path"] == "src/new.rs")
        .expect("added file");
    let deleted = files
        .iter()
        .find(|file| file["path"] == "src/gone.rs")
        .expect("deleted file");
    assert_eq!(updated["hashline"]["path"], "src/lib.rs");
    assert_eq!(
        updated["hashline"]["tag"],
        axon_agent_runtime::hashline_snapshot_tag("pub fn edited() {}\n")
    );
    assert_eq!(added["hashline"]["path"], "src/new.rs");
    assert_eq!(
        added["hashline"]["tag"],
        axon_agent_runtime::hashline_snapshot_tag("pub fn new() {}\n")
    );
    assert_eq!(updated["seenLines"], serde_json::json!([1]));
    assert_eq!(added["seenLines"], serde_json::json!([1]));
    assert!(deleted.get("hashline").is_none());
    assert!(metadata["lsp"]["files"].is_array());
    let updated_tag = updated["hashline"]["tag"].as_str().unwrap();
    let updated_path = workspace.join("src").join("lib.rs").canonicalize().unwrap();
    assert!(matches!(
        orchestrator_context.workspace_files.lookup(&updated_path, updated_tag),
        axon_agent_runtime::WorkspaceFileSnapshotLookup::Found(snapshot)
            if snapshot.seen_lines() == &std::collections::BTreeSet::from([1])
    ));
    let added_tag = added["hashline"]["tag"].as_str().unwrap();
    let added_path = workspace.join("src").join("new.rs").canonicalize().unwrap();
    assert!(matches!(
        orchestrator_context.workspace_files.lookup(&added_path, added_tag),
        axon_agent_runtime::WorkspaceFileSnapshotLookup::Found(snapshot)
            if snapshot.seen_lines() == &std::collections::BTreeSet::from([1])
    ));
    assert!(matches!(
        orchestrator_context
            .workspace_files
            .lookup(&workspace.join("src").join("gone.rs"), gone_snapshot.tag(),),
        axon_agent_runtime::WorkspaceFileSnapshotLookup::Missing
    ));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_glob_does_not_guess_files_to_preheat() {
    let workspace = temp_workspace("glob-preheat");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(workspace.join("src").join("main.rs"), "fn main() {}\n").unwrap();

    let receipt = execute_glob(
        &tool_call(
            "glob-preheat",
            &serde_json::json!({"paths":["src"]}),
            "builtin.glob",
        ),
        &context(&workspace),
    );
    assert_eq!(receipt.status, "completed");
    let metadata = receipt.metadata.expect("glob 应返回 metadata");
    assert!(metadata.get("preheat").is_none());
    assert_eq!(metadata["entries"][0]["path"], "src/main.rs");
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn execute_glob_returns_literal_file() {
    let workspace = temp_workspace("glob-literal-file");
    fs::write(workspace.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();

    let receipt = execute_glob(
        &tool_call(
            "glob-literal-file",
            &serde_json::json!({"paths":["Cargo.toml"]}),
            "builtin.glob",
        ),
        &context(&workspace),
    );

    assert_eq!(receipt.status, "completed");
    assert_eq!(
        receipt.metadata.unwrap()["entries"][0]["path"],
        "Cargo.toml"
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn provider_absolute_path_is_not_rejected_by_tool_layer() {
    let workspace = temp_workspace("read-provider-root");
    let outside = temp_workspace("read-provider-root-target");
    fs::write(outside.join("note.txt"), "outside\n").unwrap();
    let context = context(&workspace);

    let receipt = execute_read_file(
        &tool_call(
            "read-provider-root",
            &serde_json::json!({
                "path": outside.join("note.txt").display().to_string()
            }),
            "builtin.read_file",
        ),
        &context,
    );

    assert_eq!(receipt.status, "completed");
    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(outside);
}

pub(crate) fn tool_call(
    id: &str,
    arguments: &Value,
    name: &str,
) -> axon_agent_runtime::ProviderToolCall {
    axon_agent_runtime::ProviderToolCall::new(id, name, arguments.to_string())
}

pub(crate) fn context(cwd: &Path) -> ToolOrchestratorContext {
    ToolOrchestratorContext {
        config_path: None,
        cwd: cwd.to_path_buf(),
        session_id: SessionId(1),
        thread_session_id: "test-thread".to_owned(),
        cell_id: CellId(1),
        model_messages: Vec::new(),
        execution_tools: std::sync::Arc::new(std::sync::RwLock::new(
            axon_agent_runtime::ToolRegistry::new(),
        )),
        memory_write_runtime: std::sync::Arc::new(
            crate::run_commands::AgentMemoryWriteRuntime::default(),
        ),
        timeout: Duration::from_secs(15),
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        run_cancel_token: None,
        model_profile_id: axon_domain::ModelProfileId(0),
        supports_vision: true,
        permission_snapshot_id: axon_domain::PermissionSnapshotId(0),
        current_agent_path: axon_agent_runtime::AgentPath::root(),
        agent_control: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: std::sync::Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor: std::sync::Arc::new(
            crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor,
        ),
        subagent_cancel_tokens: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        user_question_port: None,
    }
}

pub(crate) fn temp_workspace(label: &str) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    let workspace = axon_test_support::test_temp_root().join(format!(
        "axon-tool-orchestrator-fs-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&workspace).unwrap();
    workspace
}
