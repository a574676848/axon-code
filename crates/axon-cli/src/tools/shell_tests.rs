use super::*;

#[test]
fn shell_post_run_lsp_metadata_uses_detected_write_paths() {
    let root = temp_workspace("shell-post-run-lsp");
    let output = root.join("out.txt");
    std::fs::write(&output, "updated\n").unwrap();
    let mut tool_receipt = ToolReceipt {
        tool_call_id: 1,
        part_id: "part".to_owned(),
        provider_call_id: "call".to_owned(),
        tool_name: "builtin.shell_command".to_owned(),
        status: "completed".to_owned(),
        summary: "shell".to_owned(),
        metadata: Some(json!({"kind": "shell_command"})),
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: None,
    };
    attach_post_run_lsp(
        &mut tool_receipt,
        &root.display().to_string(),
        std::slice::from_ref(&output),
    );

    assert!(tool_receipt.filesystem_write_performed);
    assert!(tool_receipt.summary.contains("post_run_lsp_scheduled=0"));
    let metadata = tool_receipt.metadata.expect("metadata");
    assert_eq!(
        metadata["writeIntent"]["affectedPaths"][0],
        output.display().to_string()
    );
    assert_eq!(metadata["postRunLsp"]["status"], "skipped");
    assert_eq!(metadata["postRunLsp"]["files"], json!([]));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn attach_output_archive_rejects_non_object_metadata() {
    let mut tool_receipt = ToolReceipt {
        tool_call_id: 1,
        part_id: "part".to_owned(),
        provider_call_id: "call".to_owned(),
        tool_name: "builtin.shell_command".to_owned(),
        status: "completed".to_owned(),
        summary: "shell".to_owned(),
        metadata: Some(json!("invalid metadata")),
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: None,
    };
    let output_ref = OutputRef {
        id: axon_domain::OutputRefId(1),
        byte_len: 1,
        sha256: "a".repeat(64),
        source_trace: "tool://builtin/shell_command".to_owned(),
        preview: "x".to_owned(),
        archive_ref: None,
    };

    let error = attach_output_archive(&mut tool_receipt, output_ref, std::path::Path::new("."))
        .expect_err("非对象 metadata 不得静默丢失 archiveRef");
    assert!(error.contains("metadata 必须是对象"));
    assert!(tool_receipt.output_ref.is_none());
}

#[test]
fn shell_receipt_archives_existing_full_output() {
    let root = temp_workspace("shell-output-archive");
    let source = root.join("full-output.log");
    let full_output = "完整 shell 输出\n第二行\n";
    std::fs::write(&source, full_output).unwrap();
    let mut context = test_context(root.clone());
    context.config_path = Some(root.join("config.toml"));
    let tool_call = ProviderToolCall::new(
        "call-shell-archive",
        "builtin.shell_command",
        serde_json::json!({"command": "cargo test"}).to_string(),
    );
    let receipt = axon_agent_runtime::ShellCommandExecutionReceipt {
        status: axon_agent_runtime::ShellCommandExecutionStatus::Completed,
        session_id: None,
        command: vec!["cargo".to_owned(), "test".to_owned()],
        cwd: root.display().to_string(),
        exit_code: Some(0),
        timed_out: false,
        cancelled: false,
        tty: false,
        duration_ms: 12,
        stdout: axon_agent_runtime::ShellCommandBoundedStream {
            preview: "完整 shell 输出".to_owned(),
            truncated: true,
            original_chars: full_output.chars().count(),
            retained_chars: "完整 shell 输出".chars().count(),
            sha256: axon_crypto::sha256_hex(full_output.as_bytes()),
        },
        stderr: axon_agent_runtime::ShellCommandBoundedStream {
            preview: String::new(),
            truncated: false,
            original_chars: 0,
            retained_chars: 0,
            sha256: axon_crypto::sha256_hex(b""),
        },
        original_output_tokens: 8,
        filtered_output_tokens: 4,
        output_savings_pct: 50,
        output_handler_used: "compact".to_owned(),
        full_output_path: Some(source.display().to_string()),
        artifact_truncated: false,
        artifact_failed: false,
        output_generation: 0,
        changed_paths: Vec::new(),
        remote_trigger_allowed: false,
        interaction: None,
        reasons: Vec::new(),
    };

    let output_ref = archive_shell_output(&context, &tool_call, &receipt, "完整 shell 输出")
        .expect("已有完整产物应成功归档")
        .expect("完整产物应返回 OutputRef");
    assert_eq!(output_ref.source_trace, "tool://builtin/shell_command");
    let config_path = context.config_path.as_deref().expect("测试配置路径应存在");
    let data_root = crate::runtime_paths::runtime_axon_data_path_static(config_path)
        .expect("测试 Axon data 目录应可解析");
    assert_eq!(
        ContentArchiveRuntime::default()
            .read_output(&data_root, &output_ref)
            .unwrap(),
        full_output.as_bytes()
    );
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .expect("测试 Axon 运行态根应可解析");
    assert!(
        ContentArchiveRuntime::default()
            .read_output(&axon_root, &output_ref)
            .is_err(),
        "OutputRef 不应直接依赖 Axon 根目录解析"
    );
    let log_root = crate::runtime_paths::runtime_axon_log_path_static(config_path)
        .expect("测试日志目录应可解析");
    assert!(
        ContentArchiveRuntime::default()
            .read_output(&log_root, &output_ref)
            .is_err(),
        "OutputRef 不应再依赖日志根解析"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn archive_failure_projects_to_blocked_receipt() {
    let root = temp_workspace("shell-output-archive-failure");
    let source = root.join("full-output.log");
    std::fs::write(&source, "完整输出").unwrap();
    let context = test_context(root.clone());
    let tool_call = ProviderToolCall::new(
        "call-shell-archive-failure",
        "builtin.shell_command",
        serde_json::json!({"command": "cargo test"}).to_string(),
    );
    let receipt = axon_agent_runtime::ShellCommandExecutionReceipt {
        status: axon_agent_runtime::ShellCommandExecutionStatus::Completed,
        session_id: None,
        command: vec!["cargo".to_owned(), "test".to_owned()],
        cwd: root.display().to_string(),
        exit_code: Some(0),
        timed_out: false,
        cancelled: false,
        tty: false,
        duration_ms: 1,
        stdout: axon_agent_runtime::ShellCommandBoundedStream {
            preview: "完整输出".to_owned(),
            truncated: true,
            original_chars: 4,
            retained_chars: 4,
            sha256: axon_crypto::sha256_hex("完整输出".as_bytes()),
        },
        stderr: axon_agent_runtime::ShellCommandBoundedStream {
            preview: String::new(),
            truncated: false,
            original_chars: 0,
            retained_chars: 0,
            sha256: axon_crypto::sha256_hex(b""),
        },
        original_output_tokens: 2,
        filtered_output_tokens: 1,
        output_savings_pct: 50,
        output_handler_used: "compact".to_owned(),
        full_output_path: Some(source.display().to_string()),
        artifact_truncated: false,
        artifact_failed: false,
        output_generation: 0,
        changed_paths: Vec::new(),
        remote_trigger_allowed: false,
        interaction: None,
        reasons: Vec::new(),
    };

    let error = project_shell_receipt(&tool_call, &context, &receipt)
        .expect_err("缺少 config path 时不得静默返回摘要成功");
    let blocked = blocked_tool_receipt(&tool_call, error);
    assert_eq!(blocked.status, "blocked");
    assert_eq!(blocked.error_type.as_deref(), Some("blocked"));
    assert!(blocked.summary.contains("阻断"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn shell_apply_patch_projects_structured_diff_receipt() {
    let root = temp_workspace("shell-apply-patch");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("lib.rs"), "pub fn old() {}\n").unwrap();
    std::fs::write(root.join("src").join("gone.rs"), "pub fn gone() {}\n").unwrap();
    let script = "*** Begin Patch\n\
        *** Add File: src/new.rs\n\
        +pub fn new() {}\n\
        *** Update File: src/lib.rs\n\
        @@\n\
        -pub fn old() {}\n\
        +pub fn edited() {}\n\
        *** Delete File: src/gone.rs\n\
        *** End Patch";
    let command = shell_apply_patch_command(script).expect("shell command");
    let tool_call = ProviderToolCall::new(
        "call-shell-patch",
        "builtin.shell_command",
        serde_json::json!({"command": "unused"}).to_string(),
    );
    let request = ToolExecutionRequest {
        tool_call: tool_call.clone(),
        tool_call_id: axon_domain::ToolCallId(1),
        part_id: "shell-apply-patch".to_owned(),
        execution_request_id: axon_domain::ExecutionRequestId(1),
        kind: crate::tool_orchestrator::ToolKind::Edit,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        execution_permission: axon_domain::ToolExecutionPermission::Allowed,
    };
    let context = test_context(root.clone());

    let mut output_observer = None;
    let receipt =
        execute_shell_apply_patch(&request, &context, &mut output_observer, &command, &root)
            .expect("shell apply_patch 应投射为结构化回执");

    assert_eq!(receipt.status, "completed");
    assert_eq!(receipt.tool_name, "builtin.shell_command");
    assert!(receipt.filesystem_write_performed);
    let metadata = receipt.metadata.expect("metadata");
    assert_eq!(metadata["kind"], "edit");
    assert_eq!(metadata["strategy"], "apply_patch");
    assert_eq!(metadata["operationsApplied"], 3);
    assert!(
        metadata["files"].as_array().is_some_and(|files| files
            .iter()
            .any(|file| file["path"] == "src/new.rs"
                && file["operation"] == "add"
                && file["diff"] == "pub fn new() {}\n"))
    );
    assert!(
        metadata["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| file["path"] == "src/lib.rs"
                && file["operation"] == "update"
                && file["hashline"]["tag"]
                    == axon_agent_runtime::hashline_snapshot_tag("pub fn edited() {}\n")
                && file["diff"]
                    .as_str()
                    .is_some_and(|diff| diff.contains("-pub fn old() {}")
                        && diff.contains("+pub fn edited() {}"))))
    );
    assert!(
        metadata["files"].as_array().is_some_and(|files| files
            .iter()
            .any(|file| file["path"] == "src/gone.rs"
                && file["operation"] == "delete"
                && file["diff"] == "pub fn gone() {}\n"))
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src").join("new.rs")).unwrap(),
        "pub fn new() {}\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src").join("lib.rs")).unwrap(),
        "pub fn edited() {}\n"
    );
    assert!(!root.join("src").join("gone.rs").exists());
    let _ = std::fs::remove_dir_all(root);
}

fn temp_workspace(tag: &str) -> std::path::PathBuf {
    let mut root = axon_test_support::test_temp_root();
    root.push(format!(
        "axon_shell_patch_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn test_context(cwd: std::path::PathBuf) -> ToolOrchestratorContext {
    ToolOrchestratorContext {
        config_path: None,
        cwd,
        session_id: axon_domain::SessionId(1),
        thread_session_id: "test-thread".to_owned(),
        cell_id: axon_domain::CellId(1),
        model_messages: Vec::new(),
        execution_tools: std::sync::Arc::new(std::sync::RwLock::new(
            axon_agent_runtime::ToolRegistry::new(),
        )),
        memory_write_runtime: std::sync::Arc::new(
            crate::run_commands::AgentMemoryWriteRuntime::default(),
        ),
        timeout: std::time::Duration::from_secs(15),
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        run_cancel_token: None,
        model_profile_id: axon_domain::ModelProfileId(0),
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

#[cfg(windows)]
fn shell_apply_patch_command(script: &str) -> axon_domain::AxonResult<Vec<String>> {
    ShellCommandToolRuntime::command_from_script(&format!("@'\n{script}\n'@ | apply_patch"), false)
}

#[cfg(unix)]
fn shell_apply_patch_command(script: &str) -> axon_domain::AxonResult<Vec<String>> {
    ShellCommandToolRuntime::command_from_script(
        &format!("apply_patch <<'PATCH'\n{script}\nPATCH"),
        false,
    )
}
