//! `intent` 展示契约的执行侧接受性用例。

use super::*;

use super::super::orchestrator_notes_history::{execute_history_tool, execute_notes_tool};
use super::test_support::{contains_input_rejection, make_test_context};

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
        "notes_read_file",
        "notes_list_files_by_prefix",
        "notes_search_contents",
        "send_message",
        "delegate_to_fast",
        "spawn_agent",
        "goal_create",
        "goal_list",
        "goal_get",
        "goal_update",
        "goal_cancel",
        "goal_start",
        "memory_search",
        "memory_write",
        "turn_trace",
        "list_agents",
        "list_mcp_resources",
        "history_list_windows",
        "history_list_items",
        "history_read_item",
        "history_search_contents",
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
        (
            "builtin.notes_read_file",
            r#"{"intent":"回读交接便签","path":"checkpoint.md"}"#,
        ),
        (
            "builtin.notes_list_files_by_prefix",
            r#"{"intent":"查看便签文件清单"}"#,
        ),
        (
            "builtin.notes_search_contents",
            r#"{"intent":"检索便签里的验收命令","query":"cargo"}"#,
        ),
        (
            "builtin.memory_search",
            r#"{"intent":"回忆既有决策","query":"仓库文档重整的偏好与约束"}"#,
        ),
        (
            "builtin.memory_write",
            r#"{"intent":"固化并行开发纪律","kind":"preference","summary":"并行改动必须隔离工作树","evidence":"用户明确要求"}"#,
        ),
        (
            "builtin.turn_trace",
            r#"{"intent":"核对上一轮回执原文","receipt_ref":"receipt:1","max_chars":800}"#,
        ),
        (
            "builtin.history_list_windows",
            r#"{"intent":"找回上次构建失败原因","limit":5}"#,
        ),
        (
            "builtin.history_list_items",
            r#"{"intent":"定位失败命令所在条目","window_id":"win_1","limit":10}"#,
        ),
        (
            "builtin.history_read_item",
            r#"{"intent":"回读失败命令输出","window_id":"win_1","item_id":"win_1:12"}"#,
        ),
        (
            "builtin.history_search_contents",
            r#"{"intent":"检索构建失败的报错片段","query":"error[E0"}"#,
        ),
        (
            "builtin.list_mcp_resources",
            r#"{"intent":"盘点可用的 MCP 资源"}"#,
        ),
    ];
    for (tool_name, arguments) in dispatched {
        let call = ProviderToolCall::new("call_intent_probe", tool_name, arguments);
        let receipt = match tool_name {
            "builtin.update_plan" => execute_update_plan(&call),
            "builtin.glob" => crate::tool_orchestrator_fs::execute_glob(&call, &context),
            "builtin.search" => execute_search(&call, &context),
            "builtin.memory_search" => execute_memory_search(&call, &context),
            "builtin.memory_write" => execute_memory_write(&call, &context),
            "builtin.list_mcp_resources" => {
                crate::tool_orchestrator_runtime_capabilities::execute_list_mcp_resources(
                    &call, &context,
                )
            }
            name if name.starts_with("builtin.history_") => execute_history_tool(&call, &context),
            "builtin.turn_trace" => execute_turn_trace(&call, &context),
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
                    r#"{"task_name":"intent_probe","task":"检查 intent 接受性","intent":"验证 intent 接受性"}"#,
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

    // 代理巡检同样是 intent 工具：折叠态只显示这一句话。
    let listed = subagent_runtime.execute(
        &mut control,
        axon_agent_runtime::SubagentToolExecutionInput {
            tool_call: ProviderToolCall::new(
                "call_intent_list",
                "builtin.list_agents",
                r#"{"intent":"查看并行委派的进度"}"#,
            ),
            current_path: axon_agent_runtime::AgentPath::root(),
            remote_requested: false,
            completed_at_ms: 0,
        },
    );
    assert!(
        listed.is_ok(),
        "list_agents 不得拒绝 intent：{:?}",
        listed.err()
    );

    // 委托快速代理同样声明 intent：折叠态只展示这一句话，执行侧必须接受它。
    let delegated = subagent_runtime.execute(
        &mut control,
        axon_agent_runtime::SubagentToolExecutionInput {
            tool_call: ProviderToolCall::new(
                "call_intent_delegate",
                "builtin.delegate_to_fast",
                r#"{"task":"只读审计 docs 的展示契约","intent":"审计文档展示契约"}"#,
            ),
            current_path: axon_agent_runtime::AgentPath::root(),
            remote_requested: false,
            completed_at_ms: 0,
        },
    );
    assert!(
        delegated.is_ok(),
        "delegate_to_fast 不得拒绝 intent：{:?}",
        delegated.err()
    );

    // 六个 Goal 工具同属 intent 工具：展示字段不得堵死任务管理通道。
    for (tool_name, arguments) in [
        (
            "builtin.goal_create",
            r#"{"intent":"拆解记忆增强目标","scope":"current_workspace","title":"记忆增强","objective":"落地可交付清单","steps":["梳理"],"acceptance":["清单可验收"]}"#,
        ),
        (
            "builtin.goal_list",
            r#"{"intent":"盘点进行中的目标","scope":"current_context"}"#,
        ),
        (
            "builtin.goal_get",
            r#"{"intent":"确认目标进度","goalId":"goal-1"}"#,
        ),
        (
            "builtin.goal_update",
            r#"{"intent":"标记等待输入","goalId":"goal-1","expectedRevision":1,"waitingReason":"等待确认"}"#,
        ),
        (
            "builtin.goal_cancel",
            r#"{"intent":"取消过期目标","goalId":"goal-1","expectedRevision":1,"reason":"需求变更"}"#,
        ),
        (
            "builtin.goal_start",
            r#"{"intent":"继续执行目标","goalId":"goal-1","expectedRevision":1}"#,
        ),
    ] {
        let parsed: serde_json::Value =
            serde_json::from_str(arguments).expect("用例参数是合法 JSON");
        assert!(
            axon_agent_runtime::GoalToolRuntime::parse_command(tool_name, &parsed).is_ok(),
            "{tool_name} 不得拒绝 intent"
        );
    }

    let _ = std::fs::remove_dir_all(&temp_root);
}
