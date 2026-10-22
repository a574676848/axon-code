use super::*;
use axon_output_filter::lsp_symbol_filter::{LspFilterLevel, TokenSavings};

#[test]
fn generic_tool_receipt_projections_use_runtime_status_contract() {
    let tool_call = ProviderToolCall::new("call-1", "builtin.search", "{}");

    let completed = agent_run_completed_tool_receipt(
        &tool_call,
        "completed locally",
        Some(serde_json::json!({"kind": "search"})),
        false,
    );
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.error_type, None);
    assert_eq!(completed.summary, "completed locally");
    assert_eq!(
        completed.metadata,
        Some(serde_json::json!({"kind": "search"}))
    );
    assert!(!completed.remote_trigger_allowed);
    assert!(!completed.filesystem_write_performed);

    let blocked = agent_run_blocked_tool_receipt(&tool_call, "blocked locally");
    assert_eq!(blocked.status, "blocked");
    assert_eq!(blocked.error_type.as_deref(), Some("blocked"));
    assert_eq!(blocked.summary, "blocked locally");
    assert_eq!(blocked.metadata, None);
    assert!(!blocked.remote_trigger_allowed);
    assert!(!blocked.filesystem_write_performed);

    let failed = agent_run_failed_tool_receipt(&tool_call, "failed locally");
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.error_type.as_deref(), Some("execution_failed"));
    assert_eq!(failed.summary, "failed locally");
    assert_eq!(failed.metadata, None);
    assert!(!failed.remote_trigger_allowed);
    assert!(!failed.filesystem_write_performed);

    let invalid = agent_run_failed_tool_receipt_with_error_type(
        &tool_call,
        "path 不是目录，请改用父目录或 read_file",
        AGENT_RUN_TOOL_ERROR_TYPE_INVALID_ARGUMENTS,
    );
    assert_eq!(invalid.status, "failed");
    assert_eq!(
        invalid.error_type.as_deref(),
        Some(AGENT_RUN_TOOL_ERROR_TYPE_INVALID_ARGUMENTS)
    );
}

/// R-27 回归：归档**写入失败**时 `fullOutputAvailable` 必须为假。
///
/// `full_output_path` 为 `Some` 只说明路径存在，不代表文件完整：写入失败
/// （磁盘写满 / 目录不可写 / 卷只读）时归档只含前缀。作业通道本就两个标志都查
/// （`job_runtime`），shell 通道曾只查截断位，导致回执在最需要诚实的降级场景里撒谎
/// （`INV-3`）。
#[test]
fn shell_receipt_marks_full_output_unavailable_when_archive_write_failed() {
    let tool_call = ProviderToolCall::new("call-1", "builtin.shell_command", "{}");
    let mut receipt = shell_receipt(ShellCommandExecutionStatus::Completed);
    receipt.full_output_path = Some("logs/tee/session.shell.log".to_owned());
    receipt.artifact_failed = true;

    let projection = agent_run_shell_tool_receipt(&tool_call, &receipt);
    let metadata = projection.metadata.expect("shell 回执必须携带 metadata");

    assert_eq!(
        metadata["output"]["fullOutputAvailable"],
        serde_json::json!(false),
        "归档写入失败时不得宣称完整输出可用"
    );
    assert_eq!(
        metadata["output"]["artifactFailed"],
        serde_json::json!(true)
    );
    assert!(
        projection.summary.contains("full_output_failed=true"),
        "摘要必须暴露归档写入失败：{}",
        projection.summary
    );

    // 反向对照：仅当「路径存在 + 未截断 + 未失败」时才为真：
    // 缺了这条对照，把判据改成恒假也能让上一个断言通过。
    let mut healthy = shell_receipt(ShellCommandExecutionStatus::Completed);
    healthy.full_output_path = Some("logs/tee/session.shell.log".to_owned());
    let healthy_metadata = agent_run_shell_tool_receipt(&tool_call, &healthy)
        .metadata
        .expect("shell 回执必须携带 metadata");
    assert_eq!(
        healthy_metadata["output"]["fullOutputAvailable"],
        serde_json::json!(true),
        "健康归档必须仍报可用，否则修复方向反了"
    );
}

#[test]
fn shell_tool_receipt_projection_uses_runtime_status_contract() {
    let tool_call = ProviderToolCall::new("call-1", "builtin.shell_command", "{}");
    let receipt = shell_receipt(ShellCommandExecutionStatus::Completed);

    let projection = agent_run_shell_tool_receipt(&tool_call, &receipt);

    assert_eq!(projection.provider_call_id, "call-1");
    assert_eq!(projection.tool_name, "builtin.shell_command");
    assert_eq!(projection.status, "completed");
    assert_eq!(projection.error_type, None);
    assert!(projection.summary.contains("stdout_truncated=true"));
    assert!(projection.summary.contains("stdout_chars=4/10"));
    assert!(projection.summary.contains("output_handler=generic"));
    assert!(projection.summary.contains("output_savings_pct=60"));
    assert!(projection.summary.contains("stderr_truncated=false"));
    assert!(
        projection.summary.find("stdout_truncated=true").unwrap()
            < projection.summary.find("stdout=axon").unwrap()
    );
    let metadata = projection.metadata.as_ref().expect("shell metadata");
    assert_eq!(metadata["kind"], "exec");
    assert_eq!(metadata["toolName"], "shell_command");
    assert_eq!(metadata["status"], "completed");
    assert_eq!(metadata["argumentsPreview"], "sh -c echo axon");
    assert_eq!(metadata["outputPreview"], "axon");
    assert_eq!(metadata["exitCode"], 0);
    assert_eq!(metadata["durationMs"], 1);
    assert_eq!(metadata["output"]["stdout"]["preview"], "axon");
    assert_eq!(metadata["output"]["stdout"]["truncated"], true);
    assert_eq!(metadata["output"]["tokens"]["savingsPct"], 60);
    assert_eq!(metadata["terminalInteraction"], serde_json::Value::Null);
    assert!(!projection.remote_trigger_allowed);
    assert!(!projection.filesystem_write_performed);

    let failed = agent_run_shell_tool_receipt(
        &tool_call,
        &shell_receipt(ShellCommandExecutionStatus::Failed),
    );
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.error_type.as_deref(), Some("execution_failed"));

    let blocked = agent_run_shell_tool_receipt(
        &tool_call,
        &shell_receipt(ShellCommandExecutionStatus::Blocked),
    );
    assert_eq!(blocked.status, "blocked");
    assert_eq!(blocked.error_type.as_deref(), Some("blocked"));
}

#[test]
fn write_stdin_receipt_projects_terminal_interaction_metadata() {
    let tool_call = ProviderToolCall::new("call-stdin", "builtin.write_stdin", "{}");
    let mut receipt = shell_receipt(ShellCommandExecutionStatus::Running);
    receipt.session_id = Some("session-1".to_owned());
    receipt.interaction = Some(ShellCommandInteraction {
        kind: ShellCommandInteractionKind::Input,
        stdin: "yes\n".to_owned(),
    });

    let projection = agent_run_shell_tool_receipt(&tool_call, &receipt);
    let metadata = projection.metadata.expect("write_stdin metadata");
    assert_eq!(metadata["argumentsPreview"], "sh -c echo axon");
    assert_eq!(metadata["terminalInteraction"]["kind"], "input");
    assert_eq!(metadata["terminalInteraction"]["stdin"], "yes\n");
}

#[test]
fn search_tool_receipt_projection_uses_runtime_status_contract() {
    let tool_call = ProviderToolCall::new("call-1", "builtin.search", "{}");
    let receipt = search_receipt(SearchToolExecutionStatus::Completed);

    let projection = agent_run_search_tool_receipt(&tool_call, &receipt);

    assert_eq!(projection.provider_call_id, "call-1");
    assert_eq!(projection.tool_name, "builtin.search");
    assert_eq!(projection.status, "completed");
    assert_eq!(projection.error_type, None);
    assert_eq!(
        projection.summary,
        "search query=axon files=1 files_scanned=2 next_file_offset=1 incomplete=false missing=0 binary=0 unreadable=0\nsrc/lib.rs matches=1 truncated=true hashline=A1B2\n1:A1B2 axon"
    );
    let metadata = projection.metadata.expect("search metadata");
    assert_eq!(metadata["kind"], "search");
    assert_eq!(metadata["files"][0]["hashlineTag"], "A1B2");
    assert_eq!(metadata["files"][0]["lines"][0]["isMatch"], true);
    assert!(!projection.remote_trigger_allowed);
    assert!(!projection.filesystem_write_performed);

    let blocked = agent_run_search_tool_receipt(
        &tool_call,
        &search_receipt(SearchToolExecutionStatus::Blocked),
    );
    assert_eq!(blocked.status, "blocked");
    assert_eq!(blocked.error_type.as_deref(), Some("blocked"));
}

#[test]
fn fs_tool_receipt_projections_use_runtime_metadata_contract() {
    let read_call = ProviderToolCall::new("call-1", "builtin.read_file", "{}");
    let read_receipt = read_file_receipt();

    let read_projection = agent_run_read_file_tool_receipt(&read_call, &read_receipt);

    assert_eq!(read_projection.status, "completed");
    assert_eq!(
        read_projection.summary,
        "read_file path=src/lib.rs content_kind=text hashline=[src/lib.rs#A1B2] lines=1-1 total_lines=1 next_offset=end bytes=6 truncated=false filter_level=none\n1:fn main() {}"
    );
    let read_metadata = read_projection.metadata.expect("read_file metadata");
    assert_eq!(read_metadata["kind"], "read_file");
    assert!(read_metadata.get("preview").is_none());
    assert_eq!(read_metadata["hashline"]["path"], "src/lib.rs");
    assert_eq!(read_metadata["hashline"]["tag"], "A1B2");
    assert_eq!(read_metadata["hashline"]["header"], "[src/lib.rs#A1B2]");
    assert_eq!(read_metadata["page"]["lineStart"], 1);
    assert_eq!(read_metadata["page"]["nextOffset"], serde_json::Value::Null);
    assert!(read_metadata.get("lspReceipt").is_none());

    let glob_call = ProviderToolCall::new("call-2", "builtin.glob", "{}");
    let glob_receipt = glob_receipt();

    let glob_projection = agent_run_glob_tool_receipt(&glob_call, &glob_receipt);

    assert_eq!(glob_projection.status, "completed");
    assert_eq!(
        glob_projection.summary,
        "glob root=. entries=1 next_offset=end truncated=false incomplete=false missing=0 unreadable=0\nsrc/lib.rs"
    );
    let glob_metadata = glob_projection.metadata.expect("glob metadata");
    assert_eq!(glob_metadata["kind"], "glob");
    assert_eq!(glob_metadata["entries"][0]["path"], "src/lib.rs");
    assert_eq!(glob_metadata["page"]["nextOffset"], serde_json::Value::Null);
    assert!(glob_metadata.get("preheat").is_none());
}

#[test]
fn edit_tool_receipt_projection_uses_unified_metadata_contract() {
    let projection = agent_run_edit_tool_receipt(AgentRunEditToolReceiptInput {
        tool_call: ProviderToolCall::new("call-1", "builtin.edit", "{}"),
        strategy: "apply_patch".to_owned(),
        paths: vec!["src/lib.rs".to_owned()],
        summary: "统一 edit 结构化补丁写入".to_owned(),
        files: serde_json::json!([{
            "path": "src/lib.rs",
            "diff": "@@\n-old\n+new"
        }]),
        operations_applied: 4,
        warnings: Vec::new(),
        lsp: serde_json::json!({
            "documents": [{
                "workspacePath": "src/lib.rs",
                "diagnostics": [{
                "severity": 1,
                "source": "rust-analyzer",
                "code": "E0425",
                "message": "找不到值 `missing`",
                "range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 11}}
            }, {
                "severity": 1,
                "source": "rust-analyzer",
                "code": "E0425",
                "message": "找不到值 `missing_again`",
                "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 17}}
            }]
            }],
            "refreshed": 1
        }),
    });

    assert_eq!(projection.status, "completed");
    assert_eq!(
        projection.summary,
        "edit files=1 operations_applied=4 warnings=0 summary=统一 edit 结构化补丁写入"
    );
    assert!(projection.filesystem_write_performed);
    let metadata = projection.metadata.expect("edit metadata");
    assert_eq!(metadata["kind"], "edit");
    assert_eq!(metadata["strategy"], "apply_patch");
    assert_eq!(metadata["paths"][0], "src/lib.rs");
    assert_eq!(metadata["files"][0]["path"], "src/lib.rs");
    assert_eq!(metadata["files"][0]["diff"], "@@\n-old\n+new");
    assert_eq!(metadata["diagnosticsSummary"][0]["path"], "src/lib.rs");
    assert_eq!(metadata["diagnosticsSummary"][0]["source"], "rust-analyzer");
    assert_eq!(metadata["diagnosticsClusters"][0]["path"], "src/lib.rs");
    assert_eq!(metadata["diagnosticsClusters"][0]["count"], 2);
    assert_eq!(
        metadata["diagnosticsClusters"][0]["fixHint"],
        "检查名称是否拼写正确，或补充对应作用域内的定义/import。"
    );
    assert_eq!(
        metadata["diagnosticsClusters"][0]["truncation"]["originalCount"],
        2
    );
    assert_eq!(
        metadata["diagnosticsClusters"][0]["truncation"]["retainedClusters"],
        1
    );
    assert_eq!(
        metadata["diagnosticsClusters"][0]["truncation"]["truncatedClusters"],
        0
    );
    assert_eq!(
        metadata["postApplyDiagnosticsErrors"][0]["representative"]["message"],
        "找不到值 `missing`"
    );
    assert_eq!(metadata["lsp"]["refreshed"], 1);
    assert_eq!(
        metadata["nextActions"]["snapshotHeaders"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        metadata["nextActions"]["verify"]["fixPostApplyDiagnostics"],
        true
    );
    assert_eq!(
        metadata["nextActions"]["guidance"][0],
        "后续同一文件的小范围编辑优先复用 files[].hashline.header"
    );
}

/// 自动修复必须显著可见：存在 `[修复]` warning 时，nextActions 显式要求复核 diff。
#[test]
fn edit_receipt_appends_review_guidance_when_auto_repair_happened() {
    let metadata = edit_receipt_metadata(vec![format!(
        "{}已修复 SWAP 1.=1 的双侧边界回声：移除范围外已存在的 1 行前缀和 1 行后缀",
        crate::HASHLINE_REPAIR_WARNING_PREFIX
    )]);

    let guidance = metadata["nextActions"]["guidance"]
        .as_array()
        .expect("guidance 必须是数组");
    assert_eq!(guidance.len(), 3, "存在自动修复时应追加一条复核指引");
    assert!(
        guidance[2]
            .as_str()
            .is_some_and(|text| text.contains("自动修复") && text.contains("复核")),
        "追加的指引必须要求复核自动修复：{guidance:?}"
    );
}

/// 反例守护：检测必须依据 `[修复]` 前缀，而非「warnings 非空」。
#[test]
fn edit_receipt_keeps_guidance_minimal_without_auto_repair() {
    let metadata = edit_receipt_metadata(vec!["输出被截断，预览不是完整内容".to_owned()]);

    let guidance = metadata["nextActions"]["guidance"]
        .as_array()
        .expect("guidance 必须是数组");
    assert_eq!(
        guidance.len(),
        2,
        "普通 warning 不得触发复核指引，否则检测会退化成「warnings 非空」"
    );
}

fn edit_receipt_metadata(warnings: Vec<String>) -> serde_json::Value {
    agent_run_edit_tool_receipt(AgentRunEditToolReceiptInput {
        tool_call: ProviderToolCall::new("call-repair", "builtin.edit", "{}"),
        strategy: "hashline".to_owned(),
        paths: vec!["src/lib.rs".to_owned()],
        summary: "hashline 写入".to_owned(),
        files: serde_json::json!([{"path": "src/lib.rs", "diff": "@@\n-old\n+new"}]),
        operations_applied: 1,
        warnings,
        lsp: serde_json::json!({"documents": []}),
    })
    .metadata
    .expect("edit metadata")
}

/// Running 只表示仍在运行：不得给出退出码，也不得混入会话通道字段。
#[test]
fn job_read_receipt_never_claims_success_while_running() {
    let tool_call = ProviderToolCall::new("call-job", "builtin.job_output", "{}");

    let projection =
        agent_run_job_output_tool_receipt(&tool_call, &job_output(JobStatus::Running, None), None);

    assert_eq!(projection.status, "running");
    assert_eq!(
        projection.error_type, None,
        "仍在运行既不是失败，也不能编码成成功"
    );
    let metadata = projection.metadata.expect("作业回执必须携带 metadata");
    assert_eq!(metadata["kind"], "job");
    assert_eq!(metadata["jobId"], "7");
    assert!(metadata["exitCode"].is_null(), "Running 时不得给出退出码");
    assert!(
        metadata.get("sessionId").is_none(),
        "作业通道不得出现会话通道字段：{metadata}"
    );
}

/// 终态回执必须区分「失败」与「被终止」，并携带退出码与归档可用性。
#[test]
fn job_terminal_receipt_reports_exit_code_and_honest_error_type() {
    let tool_call = ProviderToolCall::new("call-job", "builtin.job_output", "{}");

    let failed = agent_run_job_output_tool_receipt(
        &tool_call,
        &job_output(JobStatus::Failed, Some(3)),
        None,
    );
    let killed =
        agent_run_job_output_tool_receipt(&tool_call, &job_output(JobStatus::Killed, None), None);

    assert_eq!(failed.status, "failed");
    assert_eq!(failed.error_type.as_deref(), Some("execution_failed"));
    assert_eq!(failed.metadata.expect("metadata")["exitCode"], 3);
    assert_eq!(killed.status, "killed");
    assert_eq!(
        killed.error_type.as_deref(),
        Some(AGENT_RUN_TOOL_ERROR_TYPE_KILLED),
        "被终止的作业不得被报成执行失败，否则模型无法分辨两种结局"
    );
    assert_eq!(
        killed.metadata.expect("metadata")["output"]["fullOutputAvailable"],
        true
    );
}

/// 输出预算必须真正生效：超出预算时保留头尾、标记截断，并仍指向完整归档。
#[test]
fn job_read_receipt_bounds_preview_by_output_budget() {
    let tool_call = ProviderToolCall::new("call-job", "builtin.job_output", "{}");
    let mut output = job_output(JobStatus::Completed, Some(0));
    output.stdout.preview = "头".repeat(JOB_BUDGET_TEST_CHARS);
    output.stdout.original_chars = JOB_BUDGET_TEST_CHARS;

    let projection =
        agent_run_job_output_tool_receipt(&tool_call, &output, Some(JOB_BUDGET_TEST_TOKENS));
    let budget_chars = JOB_BUDGET_TEST_TOKENS * crate::builtin_shell_tool::APPROX_CHARS_PER_TOKEN;
    let stdout = &projection.metadata.expect("metadata")["output"]["stdout"];

    assert_eq!(
        stdout["preview"]
            .as_str()
            .map(|preview| preview.chars().count()),
        Some(budget_chars),
        "预算内的字符数必须被严格遵守"
    );
    assert_eq!(stdout["truncated"], true);
    assert_eq!(stdout["originalChars"], JOB_BUDGET_TEST_CHARS);
    assert!(
        stdout["preview"]
            .as_str()
            .is_some_and(|preview| preview.contains("output omitted")),
        "截断必须显式可见，而不是静默丢内容"
    );
    assert_eq!(projection.status, "completed");
}

/// 反例守护：预算足够时不得改写预览，也不得凭空标记截断。
#[test]
fn job_read_receipt_keeps_preview_within_budget_untouched() {
    let tool_call = ProviderToolCall::new("call-job", "builtin.job_output", "{}");
    let output = job_output(JobStatus::Completed, Some(0));

    let projection = agent_run_job_output_tool_receipt(&tool_call, &output, Some(2_000));
    let stdout = &projection.metadata.expect("metadata")["output"]["stdout"];

    assert_eq!(stdout["preview"], output.stdout.preview);
    assert_eq!(stdout["truncated"], output.stdout.truncated);
}

/// 启动回执只能承诺「作业已启动」，并给出作业通道的 ID。
#[test]
fn job_started_receipt_reports_job_id_only() {
    let tool_call = ProviderToolCall::new("call-shell", "builtin.shell_command", "{}");

    let projection = agent_run_job_started_tool_receipt(&tool_call, JobId(11));

    assert_eq!(projection.status, "running");
    assert_eq!(projection.error_type, None);
    let metadata = projection.metadata.expect("启动回执必须携带 metadata");
    assert_eq!(metadata["jobId"], "11");
    assert!(metadata["exitCode"].is_null());
    assert!(
        metadata.get("sessionId").is_none(),
        "后台作业不提供会话通道：{metadata}"
    );
}

fn job_output(status: JobStatus, exit_code: Option<i32>) -> JobOutput {
    JobOutput {
        job_id: JobId(7),
        label: "echo axon".to_owned(),
        elapsed_ms: 12,
        status,
        exit_code,
        stdout: ShellCommandBoundedStream {
            preview: "axon".to_owned(),
            truncated: false,
            original_chars: 4,
            retained_chars: 4,
            sha256: "stdout".to_owned(),
        },
        stderr: ShellCommandBoundedStream {
            preview: String::new(),
            truncated: false,
            original_chars: 0,
            retained_chars: 0,
            sha256: "stderr".to_owned(),
        },
        artifact_path: std::path::PathBuf::from("job-7.shell.log"),
        full_output_available: true,
    }
}

/// 预算用例的预览长度；必须显著大于折算后的字符预算。
const JOB_BUDGET_TEST_CHARS: usize = 400;
/// 预算用例的 token 预算。
const JOB_BUDGET_TEST_TOKENS: usize = 16;

fn read_file_receipt() -> ReadFileToolExecutionReceipt {
    ReadFileToolExecutionReceipt {
        status: FsToolExecutionStatus::Completed,
        path: "src/lib.rs".to_owned(),
        content_kind: crate::ReadFileContentKind::Text,
        preview: "1:fn main() {}".to_owned(),
        truncated: false,
        line_start: 1,
        line_end: 1,
        total_lines: 1,
        next_offset: None,
        returned_bytes: 6,
        truncation_reason: None,
        original_chars: 4,
        filtered_chars: 4,
        filter_level_applied: Some(LspFilterLevel::None),
        symbols_extracted: Vec::new(),
        lsp_used: false,
        token_savings: TokenSavings {
            original_tokens: 1,
            filtered_tokens: 1,
            savings_pct: 0,
        },
        content_sha256: "a1b2c3d4e5f60718293a4b5c6d7e8f90123456789abcdef0fedcba9876543210"
            .to_owned(),
        snapshot_tag: Some("A1B2".to_owned()),
        seen_lines: vec![1],
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        reasons: Vec::new(),
    }
}

fn glob_receipt() -> GlobToolExecutionReceipt {
    GlobToolExecutionReceipt {
        status: GlobToolExecutionStatus::Completed,
        root: ".".to_owned(),
        entries: vec![GlobToolEntry {
            path: "src/lib.rs".to_owned(),
            is_dir: false,
            modified_at_ms: 42,
        }],
        truncated: false,
        offset: 0,
        next_offset: None,
        missing_paths: Vec::new(),
        unreadable_paths: Vec::new(),
        incomplete: false,
        result_sha256: "abc123".to_owned(),
        remote_trigger_allowed: false,
        reasons: Vec::new(),
    }
}

fn search_receipt(status: SearchToolExecutionStatus) -> SearchToolExecutionReceipt {
    SearchToolExecutionReceipt {
        status,
        query: "axon".to_owned(),
        root: ".".to_owned(),
        files: vec![SearchToolFile {
            path: "src/lib.rs".to_owned(),
            lines: vec![SearchToolLine {
                line_number: 1,
                is_match: true,
                match_spans: vec![(0, 4)],
                text: "axon".to_owned(),
            }],
            matched_line_count: 1,
            matches_truncated: true,
            hashline_tag: Some("A1B2".to_owned()),
        }],
        files_scanned: 2,
        file_offset: 0,
        next_file_offset: Some(1),
        missing_paths: Vec::new(),
        binary_paths: Vec::new(),
        unreadable_paths: Vec::new(),
        incomplete: false,
        result_sha256: "result".to_owned(),
        remote_trigger_allowed: false,
        direct_shell_execution_performed: false,
        filesystem_write_performed: false,
        reasons: Vec::new(),
    }
}

fn shell_receipt(status: ShellCommandExecutionStatus) -> ShellCommandExecutionReceipt {
    ShellCommandExecutionReceipt {
        status,
        session_id: None,
        command: vec!["sh".to_owned(), "-c".to_owned(), "echo axon".to_owned()],
        cwd: ".".to_owned(),
        exit_code: Some(0),
        timed_out: false,
        cancelled: false,
        tty: false,
        duration_ms: 1,
        stdout: ShellCommandBoundedStream {
            preview: "axon".to_owned(),
            truncated: true,
            original_chars: 10,
            retained_chars: 4,
            sha256: "stdout".to_owned(),
        },
        stderr: ShellCommandBoundedStream {
            preview: String::new(),
            truncated: false,
            original_chars: 0,
            retained_chars: 0,
            sha256: "stderr".to_owned(),
        },
        original_output_tokens: 10,
        filtered_output_tokens: 4,
        output_savings_pct: 60,
        output_handler_used: "generic".to_owned(),
        full_output_path: None,
        artifact_truncated: false,
        artifact_failed: false,
        output_generation: 0,
        changed_paths: Vec::new(),
        remote_trigger_allowed: false,
        interaction: None,
        reasons: Vec::new(),
    }
}
