use super::*;
use std::collections::HashSet;

use crate::builtin_search_tool::SEARCH_TOOL_ID;
use crate::shell_command_intent;
use axon_domain::{ToolCallInput, ToolInputFormat};

/// 作业工具必须成对注册：只注册 `job_output` 会让模型拿到 `job_id` 后无法读取输出，
/// 只注册 `job_kill` 则无法终止长任务。
#[test]
fn job_tools_register_both_ends_of_the_background_channel() {
    let mut registry = ToolRegistry::new();
    let report = AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    let names = report
        .registered_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<HashSet<_>>();

    for expected in [crate::JOB_OUTPUT_TOOL_NAME, crate::JOB_KILL_TOOL_NAME] {
        assert!(
            names.contains(expected),
            "作业工具必须注册到模型可见集合：{expected}"
        );
    }
    for expected in ["builtin.job_output", "builtin.job_kill"] {
        assert!(
            registry
                .model_tools()
                .iter()
                .any(|tool| tool.qualified_name == expected),
            "作业工具必须对模型可见：{expected}"
        );
    }
}

#[test]
fn builtin_tools_register_model_visible_entries() {
    let mut registry = ToolRegistry::new();
    let report = AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    assert!(
        registry
            .tools()
            .iter()
            .any(|tool| tool.id == SEARCH_TOOL_ID)
    );

    assert!(
        registry
            .model_tools()
            .iter()
            .all(|tool| !tool.schema_hash.is_empty())
    );
    let spawn_agent = registry
        .tool_by_qualified_name("builtin.spawn_agent")
        .expect("builtin.spawn_agent 定义必须保留在执行注册表");
    assert!(!spawn_agent.enabled);
    assert_eq!(
        spawn_agent.concurrency_policy,
        axon_domain::ToolConcurrencyPolicy::Exclusive
    );
    assert!(
        registry
            .model_tools()
            .iter()
            .all(|tool| tool.qualified_name != "builtin.spawn_agent")
    );
    assert!(registry.model_tools().iter().any(|tool| {
        tool.qualified_name == "builtin.delegate_to_fast"
            && tool.description.contains("快速子代理")
            && !tool.schema_hash.is_empty()
    }));
    assert!(registry.model_tools().iter().any(|tool| {
        tool.qualified_name == "builtin.list_agents" && !tool.schema_hash.is_empty()
    }));
    for name in [
        "builtin.goal_create",
        "builtin.goal_list",
        "builtin.goal_get",
        "builtin.goal_start",
        "builtin.goal_update",
        "builtin.goal_cancel",
    ] {
        assert!(
            registry
                .model_tools()
                .iter()
                .any(|tool| tool.qualified_name == name)
        );
    }
    assert_task_tool_surface_is_closed(&registry);
    assert_eq!(
        registry
            .tools()
            .iter()
            .find(|tool| tool.name == "edit")
            .map(|tool| tool.permission_policy),
        Some(ToolPermissionPolicy::GovernedWrite)
    );
    let memory_write = registry
        .tools()
        .iter()
        .find(|tool| tool.name == "memory_write")
        .expect("builtin.memory_write 必须注册");
    assert_eq!(
        memory_write.permission_policy,
        ToolPermissionPolicy::GovernedWrite
    );
    assert_eq!(
        registry
            .tools()
            .iter()
            .find(|tool| tool.name == "search")
            .map(|tool| tool.permission_policy),
        Some(ToolPermissionPolicy::ReadOnly)
    );
    assert_eq!(report.registered_tools.len(), registry.tools().len());
}

/// Goal 工具面是闭集：恰好六个命令，任何其他 Task 命令名都按未注册工具稳定拒绝。
fn assert_task_tool_surface_is_closed(registry: &ToolRegistry) {
    let definitions = goal_tool_definitions();
    let names = definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            crate::GOAL_CREATE_TOOL_NAME,
            crate::GOAL_LIST_TOOL_NAME,
            crate::GOAL_GET_TOOL_NAME,
            crate::GOAL_UPDATE_TOOL_NAME,
            crate::GOAL_CANCEL_TOOL_NAME,
            crate::GOAL_START_TOOL_NAME,
        ]
    );
    for command in GOAL_TOOL_NAMES_OUTSIDE_SURFACE {
        let qualified_name = format!("builtin.{command}");
        assert!(
            !crate::GoalToolRuntime::is_goal_tool(&qualified_name),
            "{qualified_name} 不属于 Goal 工具面"
        );
        assert!(
            registry.tool_by_qualified_name(&qualified_name).is_none(),
            "{qualified_name} 不得注册"
        );
        assert!(
            registry
                .validate_input(
                    "builtin",
                    command,
                    &axon_domain::ToolInput { fields: Vec::new() }
                )
                .is_err(),
            "{qualified_name} 必须按未注册工具返回结构化拒绝"
        );
    }
}

/// 不属于目标态 Goal 工具面的命令名（反例覆盖，不含已移除工具字面名）。
const GOAL_TOOL_NAMES_OUTSIDE_SURFACE: [&str; 2] = ["task_archive", "task_unknown"];

#[test]
fn builtin_tool_contracts_have_unique_ids_and_described_json_schema_fields() {
    let mut registry = ToolRegistry::new();
    let report = AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();
    let mut ids = HashSet::new();

    for tool in &report.registered_tools {
        assert!(ids.insert(tool.id), "重复内置工具 ID: {:?}", tool.id);
        assert!(
            !tool.description.trim().is_empty(),
            "{} 缺少描述",
            tool.name
        );
        assert!(
            tool.description
                .chars()
                .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)),
            "{} 描述应使用中文句式",
            tool.name
        );
        assert_schema_properties_described(&tool.schema.input_schema, &tool.name);
    }
}

#[test]
fn builtin_provider_wire_names_are_legal_for_all_supported_function_protocols() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    for tool in registry.model_tools() {
        let wire_name = crate::provider_tool_name(&tool.qualified_name);
        assert!(
            !wire_name.is_empty() && wire_name.len() <= 64,
            "{} 的 provider wire name 长度无效：{wire_name}",
            tool.qualified_name
        );
        assert!(
            wire_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
            "{} 的 provider wire name 包含非法字符：{wire_name}",
            tool.qualified_name
        );
    }
}

#[test]
fn builtin_tool_schemas_keep_meta_inputs_narrow_and_objects_closed() {
    let mut registry = ToolRegistry::new();
    let report = AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    for tool in &report.registered_tools {
        assert_eq!(
            tool.schema.input_schema["type"], "object",
            "{} 顶层输入必须是 object",
            tool.name
        );
        assert_eq!(
            tool.schema.input_schema["additionalProperties"], false,
            "{} 必须拒绝未知顶层字段",
            tool.name
        );
        match &tool.schema.input_format {
            ToolInputFormat::JsonSchema => {}
            ToolInputFormat::Freeform { fallback_field, .. } => {
                assert!(
                    matches!(tool.name.as_str(), "edit" | "shell_command"),
                    "{} 不应使用 freeform 输入",
                    tool.name
                );
                if tool.name == "shell_command" {
                    assert_eq!(
                        tool.schema.field_names(),
                        vec![fallback_field.clone()],
                        "shell_command 的 function fallback 必须只有一个字段"
                    );
                } else {
                    assert!(tool.schema.field_names().contains(fallback_field));
                }
            }
        }

        assert!(
            tool.schema.input_schema["properties"]
                .as_object()
                .into_iter()
                .flatten()
                .all(|(_, schema)| schema["type"] != "object")
        );
    }

    let invoke_skill = registry
        .tool_by_qualified_name("builtin.invoke_skill")
        .expect("invoke_skill 必须注册");
    assert_eq!(invoke_skill.schema.field_names(), vec!["skill_name"]);

    let shell = registry
        .tool_by_qualified_name("builtin.shell_command")
        .expect("shell_command 必须注册");
    assert!(matches!(
        shell.schema.input_format,
        ToolInputFormat::JsonSchema
    ));
    assert_eq!(
        shell.schema.field_names(),
        vec![
            "command",
            "intent",
            "max_output_tokens",
            "run_in_background",
            "timeout_ms",
            "tty",
            "workdir",
            "yield_time_ms"
        ],
        "shell_command 的可选字段集合是模型可见契约：新增执行形态必须同步这条清单"
    );
    assert_eq!(
        shell_command_intent(r#"{"command":"cargo fmt","intent":" 校验代码格式 "}"#).as_deref(),
        Some("校验代码格式"),
        "折叠态概要去掉首尾空白后原样呈现"
    );
    assert_eq!(
        shell_command_intent(r#"{"command":"cargo fmt","intent":"   "}"#),
        None,
        "空概要视为未填写，界面回落到命令投影"
    );
    assert_eq!(
        shell_command_intent("not json"),
        None,
        "非 JSON 参数不得造出概要"
    );
}

fn assert_schema_properties_described(schema: &serde_json::Value, tool_name: &str) {
    if let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        for (field, property) in properties {
            assert!(
                property
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|description| !description.trim().is_empty()),
                "{tool_name}.{field} 缺少字段描述"
            );
            assert_schema_properties_described(property, tool_name);
        }
    }
    if let Some(items) = schema.get("items") {
        assert_schema_properties_described(items, tool_name);
    }
    if let Some(variants) = schema.get("anyOf").and_then(serde_json::Value::as_array) {
        for variant in variants {
            assert_schema_properties_described(variant, tool_name);
        }
    }
}

#[test]
fn model_exposes_edit_as_the_only_editor() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    let registered_names = registry
        .tools()
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(registered_names.contains(&crate::EDIT_TOOL_NAME));
    assert!(!registered_names.contains(&"apply_patch"));
    assert!(!registered_names.contains(&"apply_diff"));
    assert!(!registered_names.contains(&"hashline_edit"));

    let model_tools = registry.model_tools();
    let editor_names = model_tools
        .iter()
        .map(|tool| tool.qualified_name.as_str())
        .filter(|name| {
            matches!(
                *name,
                "builtin.edit"
                    | "builtin.apply_patch"
                    | "builtin.apply_diff"
                    | "builtin.hashline_edit"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(editor_names, vec!["builtin.edit"]);

    let edit = model_tools
        .into_iter()
        .find(|tool| tool.qualified_name == "builtin.edit")
        .expect("edit 必须对模型可见");
    let axon_domain::ToolInputFormat::Freeform {
        fallback_field,
        format,
    } = edit.input_format
    else {
        panic!("edit 必须使用 freeform 输入");
    };
    assert_eq!(fallback_field, "input");
    assert_eq!(format.format_type, "grammar");
    assert_eq!(format.syntax, "lark");
    assert!(
        format
            .definition
            .contains(r#"swap: "SWAP " line_range ":" NL body_line*"#)
    );
    assert!(format.definition.contains("body_line: (\"+\" | \"-\")"));
    assert!(format.definition.contains("SWAP.BLK "));
    assert!(format.definition.contains("*** Begin Patch"));
    assert!(format.definition.contains("*** Add File: "));
    assert!(format.definition.contains("*** Delete File: "));
    assert!(format.definition.contains("*** Update File: "));
    assert!(format.definition.contains("*** Move to: "));
    assert!(format.definition.contains("HASHLINE_HEADER"));
    assert!(format.definition.contains(r"[^\r\n]+"));
    assert!(format.definition.contains("NL?"));
}

#[test]
fn builtin_schema_validation_matches_conditional_runtime_contracts() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    assert!(
        registry
            .validate_call_input(
                "builtin.list_mcp_resources",
                &ToolCallInput::json(json!({ "cursor": "next" }))
            )
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.read_mcp_resource",
                &ToolCallInput::json(json!({ "server": "mcp", "uri": "" }))
            )
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.memory_search",
                &ToolCallInput::json(json!({ "query": "memory", "limit": 0 }))
            )
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.memory_write",
                &ToolCallInput::json(json!({
                    "kind": "experience",
                    "summary": "",
                    "evidence": "已验证"
                }))
            )
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.search",
                &ToolCallInput::json(json!({ "query": "axon", "intent": "检索 axon" }))
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.search",
                &ToolCallInput::json(
                    json!({ "query": "axon", "paths": "src", "intent": "限定范围检索" })
                )
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.glob",
                &ToolCallInput::json(json!({ "paths": "src", "intent": "列出源码目录" }))
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.memory_write",
                &ToolCallInput::json(json!({
                    "kind": "experience",
                    "summary": "稳定结论",
                    "evidence": "已验证",
                    "supersedes_memory_ids": 7
                }))
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.glob",
                &ToolCallInput::json(json!({ "include": "files", "intent": "只列文件" }))
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.glob",
                &ToolCallInput::json(
                    json!({ "include": ["files", "directories"], "intent": "文件与目录都要" })
                )
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.glob",
                &ToolCallInput::json(json!({ "include": [], "intent": "空过滤被拒绝" }))
            )
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.shell_command",
                &ToolCallInput::json(json!({
                    "command": "echo axon",
                    "intent": "验证命令行为",
                    "workdir": "nested",
                    "timeout_ms": 1000,
                    "tty": true,
                    "yield_time_ms": 0,
                    "max_output_tokens": 64
                }))
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_call_input("builtin.shell_command", &ToolCallInput::text("echo axon"))
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.write_stdin",
                &ToolCallInput::json(json!({
                    "session_id": "session-1",
                    "chars": "quit\n",
                    "ctrl_c": false,
                    "max_output_tokens": 64,
                    "intent": "退出交互会话"
                }))
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.write_stdin",
                &ToolCallInput::json(json!({
                    "session_id": "session-1",
                    "chars": "quit\n",
                    "ctrl_c": true
                }))
            )
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.glob",
                &ToolCallInput::json(json!({ "paths": "src", "intent": "列出源码文件" }))
            )
            .is_ok()
    );
    // 折叠态只显示模型自述的一句话目的：search/glob/update_plan/便签写入/终端执行都必填 intent。
    for (tool, arguments) in [
        ("builtin.search", json!({ "query": "axon" })),
        ("builtin.glob", json!({ "paths": "src" })),
        (
            "builtin.update_plan",
            json!({ "plan": [
                {"step": "步骤一", "status": "pending"},
                {"step": "步骤二", "status": "pending"},
                {"step": "步骤三", "status": "pending"}
            ] }),
        ),
        (
            "builtin.notes_write_file",
            json!({ "path": "checkpoint.md", "text": "进展" }),
        ),
        (
            "builtin.notes_append_to_file",
            json!({ "path": "checkpoint.md", "text": "进展" }),
        ),
        (
            "builtin.write_stdin",
            json!({ "session_id": "sess_123", "chars": "y\n" }),
        ),
    ] {
        assert!(
            registry
                .validate_call_input(tool, &ToolCallInput::json(arguments))
                .is_err(),
            "{tool} 缺 intent 时必须被 schema 拦下：否则折叠态只能摆参数 JSON"
        );
    }
}

#[test]
fn builtin_schemas_reject_semantically_empty_strings_before_execution() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();
    let invalid_json_calls = [
        (
            "builtin.search",
            json!({ "query": " ", "intent": "空查询被拒绝" }),
        ),
        ("builtin.memory_search", json!({ "query": "\t" })),
        (
            "builtin.memory_write",
            json!({ "kind": "experience", "summary": " ", "evidence": "已验证" }),
        ),
        ("builtin.read_file", json!({ "path": " " })),
        (
            "builtin.glob",
            json!({ "paths": ["\t"], "intent": "空路径被拒绝" }),
        ),
        ("builtin.invoke_skill", json!({ "skill_name": " " })),
        ("builtin.list_mcp_resources", json!({ "server": " " })),
        (
            "builtin.read_mcp_resource",
            json!({ "server": "mcp", "uri": " " }),
        ),
        ("builtin.delegate_to_fast", json!({ "task": " " })),
        ("builtin.list_agents", json!({ "path_prefix": " " })),
    ];

    for (tool_name, input) in invalid_json_calls {
        assert!(
            registry
                .validate_call_input(tool_name, &ToolCallInput::json(input))
                .is_err(),
            "{tool_name} 应在执行前拒绝纯空白字符串"
        );
    }
    assert!(
        registry
            .validate_call_input("builtin.edit", &ToolCallInput::text(" \r\n\t"))
            .is_err()
    );
    assert!(
        registry
            .validate_call_input("builtin.shell_command", &ToolCallInput::text(" \r\n\t"))
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.memory_write",
                &ToolCallInput::json(json!({
                    "kind": "experience",
                    "summary": "稳定结论",
                    "evidence": "已验证",
                    "supersedes_memory_ids": [7, 7]
                }))
            )
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                "builtin.spawn_agent",
                &ToolCallInput::json(json!({
                    "task_name": "Review Task",
                    "task": "检查契约"
                }))
            )
            .is_err()
    );
}

/// 折叠态规则表（`axon-display`）与真实工具 schema 必须逐项一致：
/// 名单里的工具都必须声明必填 `intent`，声明了 `intent` 的工具也必须在名单里。
#[test]
fn intent_tool_list_matches_declared_schemas() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();
    let declared = crate::tool_intent_tool_names();

    for tool in registry.tools() {
        let requires_intent = tool
            .schema
            .required_field_names()
            .iter()
            .any(|field| field == crate::TOOL_INTENT_FIELD);
        assert_eq!(
            requires_intent,
            declared.iter().any(|name| name == &tool.name),
            "{} 的 intent 契约必须与折叠态规则表一致",
            tool.name
        );
    }
    for name in declared {
        assert!(
            registry.tools().iter().any(|tool| &tool.name == name),
            "折叠态规则表里的 {name} 必须是已注册的内置工具"
        );
    }
}
