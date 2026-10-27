use super::*;

use super::super::orchestrator_notes_history::{execute_history_tool, execute_notes_tool};

#[test]
fn memory_id_array_field_parses_valid_ids_and_rejects_invalid() {
    let value = serde_json::json!({
        "supersedes_memory_ids": [7, "8079221640628927909"]
    });
    assert_eq!(
        memory_id_array_field(&value, "supersedes_memory_ids").unwrap(),
        vec![7, 8_079_221_640_628_927_909]
    );
    assert_eq!(
        memory_id_array_field(
            &serde_json::json!({"supersedes_memory_ids": 7}),
            "supersedes_memory_ids"
        )
        .unwrap(),
        vec![7]
    );
    assert!(
        memory_id_array_field(
            &serde_json::json!({"supersedes_memory_ids": [0]}),
            "supersedes_memory_ids"
        )
        .is_err()
    );
    assert!(
        memory_id_array_field(
            &serde_json::json!({"supersedes_memory_ids": [7, 7]}),
            "supersedes_memory_ids"
        )
        .is_err()
    );
}

#[test]
fn history_search_contents_dispatch_matches_text_and_tool_outputs() {
    let temp_root = axon_test_support::test_temp_root()
        .join(format!("axon_search_test_{}", std::process::id()));
    let session_id = "test_history_session";
    let session_dir = temp_root.join("sessions").join(session_id);
    let config_path = temp_root.join("config.toml");
    let _ = std::fs::create_dir_all(&session_dir);
    let _ = std::fs::write(&config_path, "");

    let tool_msg =
        axon_agent_runtime::AgentModelMessage::tool_result("call_1", "全仓编译成功 100%", true);
    let text_msg = axon_agent_runtime::AgentModelMessage::text("assistant", "正在推进压缩机制重构");
    let user_msg = axon_agent_runtime::AgentModelMessage::text("user", "Compile OK");

    let records = [
        axon_agent_runtime::ArchivedWindowRecord {
            schema_version: axon_agent_runtime::ARCHIVED_WINDOW_SCHEMA_VERSION,
            lifecycle_id: "fixture-lifecycle-1".to_owned(),
            checksum: String::new(),
            window_number: 1,
            window_id: "win_1".to_string(),
            previous_window_id: None,
            archived_at_ms: 1000,
            message: text_msg,
            message_ordinal: 0,
        },
        axon_agent_runtime::ArchivedWindowRecord {
            schema_version: axon_agent_runtime::ARCHIVED_WINDOW_SCHEMA_VERSION,
            lifecycle_id: "fixture-lifecycle-1".to_owned(),
            checksum: String::new(),
            window_number: 1,
            window_id: "win_1".to_string(),
            previous_window_id: None,
            archived_at_ms: 1000,
            message: tool_msg,
            message_ordinal: 1,
        },
        axon_agent_runtime::ArchivedWindowRecord {
            schema_version: axon_agent_runtime::ARCHIVED_WINDOW_SCHEMA_VERSION,
            lifecycle_id: "fixture-lifecycle-1".to_owned(),
            checksum: String::new(),
            window_number: 1,
            window_id: "win_1".to_string(),
            previous_window_id: None,
            archived_at_ms: 1000,
            message: user_msg,
            message_ordinal: 2,
        },
    ];
    axon_agent_runtime::WindowRolloverEngine::append_to_archive(&session_dir, &records)
        .expect("append");

    let context = make_test_context(config_path, session_id);

    // 1. 搜索普通文本：命中条目 ID、角色与原文
    let call_text = ProviderToolCall::new(
        "call_text",
        "builtin.history_search_contents",
        r#"{"query":"压缩机制"}"#,
    );
    let receipt_text = execute_history_tool(&call_text, &context);
    assert!(receipt_text.summary.contains("[win_1:0]"));
    assert!(receipt_text.summary.contains("role=assistant"));
    assert!(receipt_text.summary.contains("正在推进压缩机制重构"));

    // 2. 搜索工具执行输出：工具结果正文同样进入检索范围
    let call_tool = ProviderToolCall::new(
        "call_tool",
        "builtin.history_search_contents",
        r#"{"query":"全仓编译成功"}"#,
    );
    let receipt_tool = execute_history_tool(&call_tool, &context);
    assert!(receipt_tool.summary.contains("[win_1:1]"));
    assert!(receipt_tool.summary.contains("role=tool"));
    assert!(
        receipt_tool
            .summary
            .contains("[Tool Result]: 全仓编译成功 100%")
    );

    // 3. window_id 精准命中分片
    let call_shard_match = ProviderToolCall::new(
        "call_shard",
        "builtin.history_search_contents",
        r#"{"query":"压缩机制","window_id":"win_1"}"#,
    );
    let receipt_shard = execute_history_tool(&call_shard_match, &context);
    assert!(receipt_shard.summary.contains("正在推进压缩机制重构"));

    // 4. 未知 window_id 优雅未命中（与 Codex 一致返回空结果）
    let call_shard_miss = ProviderToolCall::new(
        "call_miss",
        "builtin.history_search_contents",
        r#"{"query":"压缩机制","window_id":"win_999"}"#,
    );
    let receipt_miss = execute_history_tool(&call_shard_miss, &context);
    assert!(receipt_miss.summary.contains("未命中任何归档内容。"));

    // 5. 字面量检索区分大小写
    let call_case_miss = ProviderToolCall::new(
        "call_case_miss",
        "builtin.history_search_contents",
        r#"{"query":"compile"}"#,
    );
    assert!(
        execute_history_tool(&call_case_miss, &context)
            .summary
            .contains("未命中任何归档内容。")
    );
    let call_case_hit = ProviderToolCall::new(
        "call_case_hit",
        "builtin.history_search_contents",
        r#"{"query":"Compile"}"#,
    );
    assert!(
        execute_history_tool(&call_case_hit, &context)
            .summary
            .contains("[win_1:2]")
    );

    let _ = std::fs::remove_dir_all(&temp_root);
}

fn make_test_context(config_path: std::path::PathBuf, session_id: &str) -> ToolOrchestratorContext {
    ToolOrchestratorContext {
        config_path: Some(config_path),
        cwd: std::env::current_dir().unwrap_or_default(),
        session_id: axon_domain::SessionId(1),
        thread_session_id: session_id.to_owned(),
        cell_id: axon_domain::CellId(1),
        model_messages: Vec::new(),
        execution_tools: std::sync::Arc::new(std::sync::RwLock::new(
            axon_agent_runtime::ToolRegistry::new(),
        )),
        memory_write_runtime: std::sync::Arc::new(
            crate::run_commands::AgentMemoryWriteRuntime::default(),
        ),
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
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
        user_question_port: None,
    }
}

#[test]
fn history_search_contents_filters_and_validates_inputs() {
    let temp_root = axon_test_support::test_temp_root()
        .join(format!("axon_test_facet_{}", uuid::Uuid::new_v4()));
    let session_dir = temp_root.join("sessions").join("session-facet-1");
    let config_path = temp_root.join("config.toml");
    let _ = std::fs::create_dir_all(&session_dir);
    let _ = std::fs::write(&config_path, "");

    let rec1 = axon_agent_runtime::ArchivedWindowRecord {
        schema_version: axon_agent_runtime::ARCHIVED_WINDOW_SCHEMA_VERSION,
        lifecycle_id: "fixture-lifecycle-1".to_owned(),
        checksum: String::new(),
        window_number: 1,
        window_id: "win-1".to_string(),
        previous_window_id: None,
        archived_at_ms: 100,
        message: axon_agent_runtime::AgentModelMessage::text("user", "Window 1 发现关键架构约束"),
        message_ordinal: 0,
    };
    axon_agent_runtime::WindowRolloverEngine::append_to_archive(&session_dir, &[rec1]).unwrap();

    let rec2 = [
        axon_agent_runtime::ArchivedWindowRecord {
            schema_version: axon_agent_runtime::ARCHIVED_WINDOW_SCHEMA_VERSION,
            lifecycle_id: "fixture-lifecycle-2".to_owned(),
            checksum: String::new(),
            window_number: 2,
            window_id: "win-2".to_string(),
            previous_window_id: Some("win-1".to_string()),
            archived_at_ms: 200,
            message: axon_agent_runtime::AgentModelMessage::text(
                "user",
                "Window 2 持续推进架构重构",
            ),
            message_ordinal: 0,
        },
        axon_agent_runtime::ArchivedWindowRecord {
            schema_version: axon_agent_runtime::ARCHIVED_WINDOW_SCHEMA_VERSION,
            lifecycle_id: "fixture-lifecycle-2".to_owned(),
            checksum: String::new(),
            window_number: 2,
            window_id: "win-2".to_string(),
            previous_window_id: Some("win-1".to_string()),
            archived_at_ms: 200,
            message: axon_agent_runtime::AgentModelMessage::text(
                "assistant",
                "Window 2 已验证架构改动",
            ),
            message_ordinal: 1,
        },
    ];
    axon_agent_runtime::WindowRolloverEngine::append_to_archive(&session_dir, &rec2).unwrap();

    let context = make_test_context(config_path, "session-facet-1");

    // 1. limit 限制命中条目数；默认按窗口由旧到新扫描
    let call_limit = ProviderToolCall::new(
        "call_limit",
        "builtin.history_search_contents",
        r#"{"query":"架构","limit":1}"#,
    );
    let receipt_limit = execute_history_tool(&call_limit, &context);
    assert!(receipt_limit.summary.contains("[win-1:0]"));
    assert!(!receipt_limit.summary.contains("win-2:"));

    // 2. recent_first 反转扫描顺序，命中最新窗口
    let call_recent = ProviderToolCall::new(
        "call_recent",
        "builtin.history_search_contents",
        r#"{"query":"架构","limit":1,"recent_first":true}"#,
    );
    let receipt_recent = execute_history_tool(&call_recent, &context);
    assert!(receipt_recent.summary.contains("[win-2:1]"));

    // 3. role 过滤
    let call_role = ProviderToolCall::new(
        "call_role",
        "builtin.history_search_contents",
        r#"{"query":"架构","role":"assistant"}"#,
    );
    let receipt_role = execute_history_tool(&call_role, &context);
    assert!(receipt_role.summary.contains("role=assistant"));
    assert!(!receipt_role.summary.contains("role=user"));

    // 4. window_id 过滤
    let call_window = ProviderToolCall::new(
        "call_window",
        "builtin.history_search_contents",
        r#"{"query":"架构","window_id":"win-1"}"#,
    );
    let receipt_window = execute_history_tool(&call_window, &context);
    assert!(receipt_window.summary.contains("[win-1:0]"));
    assert!(!receipt_window.summary.contains("win-2"));

    // 5. tool_namespace 过滤：非工具条目被排除
    let call_namespace = ProviderToolCall::new(
        "call_namespace",
        "builtin.history_search_contents",
        r#"{"query":"架构","tool_namespace":"builtin"}"#,
    );
    let receipt_namespace = execute_history_tool(&call_namespace, &context);
    assert!(receipt_namespace.summary.contains("未命中任何归档内容。"));

    // 6. 入参校验：旧契约 window_number 等未知字段被拒绝
    let call_unknown = ProviderToolCall::new(
        "call_unknown",
        "builtin.history_search_contents",
        r#"{"query":"架构","window_number":1}"#,
    );
    let receipt_unknown = execute_history_tool(&call_unknown, &context);
    assert!(receipt_unknown.summary.contains("工具入参不符合 schema"));

    // 7. 入参校验：非法 role 与 limit=0
    let call_bad_role = ProviderToolCall::new(
        "call_bad_role",
        "builtin.history_search_contents",
        r#"{"query":"架构","role":"shell"}"#,
    );
    assert!(
        execute_history_tool(&call_bad_role, &context)
            .summary
            .contains("role 只支持")
    );
    let call_zero_limit = ProviderToolCall::new(
        "call_zero_limit",
        "builtin.history_search_contents",
        r#"{"query":"架构","limit":0}"#,
    );
    assert!(
        execute_history_tool(&call_zero_limit, &context)
            .summary
            .contains("limit 必须大于 0。")
    );

    let _ = std::fs::remove_dir_all(&temp_root);
}

/// 换窗工具必须在调用边界校验当前窗口保存代际：未写入便签时只拒绝本次请求
/// 并给出可执行指令，不注入换窗标记，模型仍可在同一 turn 内自我纠正。
#[test]
fn new_context_requires_saved_state_before_requesting_rollover() {
    let temp_root = axon_test_support::test_temp_root()
        .join(format!("axon_new_context_gate_{}", std::process::id()));
    let session_id = "test_new_context_gate";
    let session_dir = temp_root.join("sessions").join(session_id);
    let config_path = temp_root.join("config.toml");
    let _ = std::fs::create_dir_all(&session_dir);
    let _ = std::fs::write(&config_path, "");
    let context = make_test_context(config_path, session_id);
    let call = ProviderToolCall::new(
        "call_new_context",
        "builtin.new_context",
        r#"{"reason":"当前阶段已完成"}"#,
    );

    let blocked = execute_new_context(&call, &context);
    assert!(blocked.summary.contains("换窗未执行"));
    assert!(blocked.summary.contains("builtin.notes_write_file"));
    assert!(
        blocked
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("contextRolloverRequested"))
            .is_none(),
        "门禁未通过时不得注入换窗标记"
    );

    axon_agent_runtime::SessionNotesRuntime::write_note_file(
        &session_dir,
        "checkpoint.md",
        "目标与下一步",
        1,
    )
    .expect("便签应写入成功");
    let accepted = execute_new_context(&call, &context);
    assert!(accepted.summary.contains("换窗请求已记录"));
    assert_eq!(
        accepted
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("contextRolloverRequested")),
        Some(&serde_json::json!(true))
    );
    let marker_count = accepted
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("modelMessages"))
        .and_then(|messages| messages.as_array())
        .map_or(0, Vec::len);
    assert_eq!(marker_count, 1, "成功回执必须携带一条换窗标记");

    let _ = std::fs::remove_dir_all(&temp_root);
}

/// 执行前参数被拒的稳定文案标记：出现任一即说明工具在执行前拒绝了这次调用。
fn contains_input_rejection(summary: &str) -> bool {
    [
        "工具入参",
        "不符合 schema",
        "包含不支持的内容",
        "无法读取",
        "unknown field",
        "missing field",
    ]
    .iter()
    .any(|marker| summary.contains(marker))
}

/// 便签写入工具的 `intent` 是 schema 必填的展示字段，执行侧的严格参数解析必须接受它。
///
/// 回归背景：该字段最初只加在 schema 上，便签写入随即把合规调用判成未知字段，
/// 换窗前的最后一笔便签写不进去，跨窗交接直接断链，模型重试三次后只能熔断。
#[test]
fn notes_write_accepts_the_required_intent_field() {
    let temp_root = axon_test_support::test_temp_root()
        .join(format!("axon_notes_intent_{}", std::process::id()));
    let session_id = "test_notes_intent";
    let session_dir = temp_root.join("sessions").join(session_id);
    let config_path = temp_root.join("config.toml");
    let _ = std::fs::create_dir_all(&session_dir);
    let _ = std::fs::write(&config_path, "");
    let context = make_test_context(config_path, session_id);

    let cases = [
        (
            "builtin.notes_write_file",
            r#"{"path":"checkpoint.md","text":"目标与下一步","intent":"保存跨窗交接便签"}"#,
        ),
        (
            "builtin.notes_append_to_file",
            r#"{"path":"checkpoint.md","text":"追加下一步动作","intent":"补充下一步动作"}"#,
        ),
    ];
    for (tool_name, arguments) in cases {
        let call = ProviderToolCall::new("call_notes_intent", tool_name, arguments);
        let receipt = execute_notes_tool(&call, &context);
        assert!(
            !contains_input_rejection(&receipt.summary),
            "{tool_name} 不得把 intent 判为非法参数：{}",
            receipt.summary
        );
        assert_eq!(
            receipt.status, "completed",
            "{tool_name} 必须真的写入便签：{}",
            receipt.summary
        );
    }

    let _ = std::fs::remove_dir_all(&temp_root);
}

/// `intent` 是展示字段，却是 schema 必填项：任何一条执行链都不得拒绝它。
///
/// 覆盖口径由展示规则表驱动：规则表里声明 `intent` 的工具必须在这里给出真实解析或派发路径，
/// 只加 schema 而漏改执行侧会被这条用例直接拦下。
#[test]
fn every_intent_tool_accepts_the_intent_field_before_execution() {
    let covered = [
        "shell_command",
        "write_stdin",
        "update_plan",
        "glob",
        "search",
        "notes_write_file",
        "notes_append_to_file",
        "send_message",
    ];
    for name in axon_display::tool_intent_tool_names() {
        assert!(
            covered.contains(&name.as_str()),
            "intent 工具 {name} 缺少执行侧参数接受性覆盖"
        );
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let shell = axon_agent_runtime::agent_run_shell_tool_args_from_provider_input(
        &axon_domain::ToolCallInput::json(serde_json::json!({
            "command": "echo axon-intent-probe",
            "intent": "验证参数解析"
        })),
        &cwd,
    );
    assert!(
        shell.is_ok(),
        "shell_command 不得拒绝 intent：{:?}",
        shell.err()
    );
    let write_stdin = axon_agent_runtime::agent_run_write_stdin_tool_input_from_provider_args(
        r#"{"session_id":"sess-intent-probe","intent":"确认构建已结束"}"#,
        "owner-intent-probe",
    );
    assert!(
        write_stdin.is_ok(),
        "write_stdin 不得拒绝 intent：{:?}",
        write_stdin.err()
    );

    let temp_root = axon_test_support::test_temp_root()
        .join(format!("axon_intent_dispatch_{}", std::process::id()));
    let session_id = "test_intent_dispatch";
    let session_dir = temp_root.join("sessions").join(session_id);
    let config_path = temp_root.join("config.toml");
    let _ = std::fs::create_dir_all(&session_dir);
    let _ = std::fs::write(&config_path, "");
    let context = make_test_context(config_path, session_id);

    let dispatched = [
        (
            "builtin.update_plan",
            r#"{"intent":"提交阶段快照","plan":[{"step":"定位","status":"pending"},{"step":"实现","status":"pending"},{"step":"验证","status":"pending"}]}"#,
        ),
        (
            "builtin.glob",
            r#"{"paths":"Cargo.toml","intent":"确认仓库清单"}"#,
        ),
        (
            "builtin.search",
            r#"{"query":"axon","intent":"确认关键字检索"}"#,
        ),
        (
            "builtin.notes_write_file",
            r#"{"path":"checkpoint.md","text":"当前进展","intent":"保存交接便签"}"#,
        ),
        (
            "builtin.notes_append_to_file",
            r#"{"path":"checkpoint.md","text":"下一步动作","intent":"补充交接便签"}"#,
        ),
    ];
    for (tool_name, arguments) in dispatched {
        let call = ProviderToolCall::new("call_intent_probe", tool_name, arguments);
        let receipt = match tool_name {
            "builtin.update_plan" => execute_update_plan(&call),
            "builtin.glob" => crate::tool_orchestrator_fs::execute_glob(&call, &context),
            "builtin.search" => execute_search(&call, &context),
            _ => execute_notes_tool(&call, &context),
        };
        assert!(
            !contains_input_rejection(&receipt.summary),
            "{tool_name} 不得拒绝 intent：{}",
            receipt.summary
        );
    }

    // 子代理协作工具同属 intent 工具：它是子代理之间唯一的消息通道，
    // 一个展示字段不得把这条通道堵死。
    let mut control = axon_agent_runtime::AgentControlRuntime::default();
    let subagent_runtime = axon_agent_runtime::SubagentToolRuntime;
    subagent_runtime
        .execute(
            &mut control,
            axon_agent_runtime::SubagentToolExecutionInput {
                tool_call: ProviderToolCall::new(
                    "call_intent_spawn",
                    "builtin.spawn_agent",
                    r#"{"task_name":"intent_probe","task":"检查 intent 接受性"}"#,
                ),
                current_path: axon_agent_runtime::AgentPath::root(),
                remote_requested: false,
                completed_at_ms: 0,
            },
        )
        .expect("spawn 必须成功");
    let sent = subagent_runtime.execute(
        &mut control,
        axon_agent_runtime::SubagentToolExecutionInput {
            tool_call: ProviderToolCall::new(
                "call_intent_send",
                "builtin.send_message",
                r#"{"target":"intent_probe","message":"继续","intent":"补充协作指令"}"#,
            ),
            current_path: axon_agent_runtime::AgentPath::root(),
            remote_requested: false,
            completed_at_ms: 0,
        },
    );
    assert!(
        sent.is_ok(),
        "send_message 不得拒绝 intent：{:?}",
        sent.err()
    );

    let _ = std::fs::remove_dir_all(&temp_root);
}
