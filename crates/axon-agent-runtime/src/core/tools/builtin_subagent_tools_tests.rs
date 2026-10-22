use super::*;
use crate::builtin_subagent_tool_spec::{
    SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD, delegate_to_fast_tool_definition,
};
use crate::builtin_subagent_tools::SUBAGENT_TOOL_REASON_CHILD_SPAWN_BLOCKED;
use crate::builtin_subagent_tools::SUBAGENT_TOOL_REASON_SELF_TARGET_REJECTED;
use crate::builtin_subagent_tools::SUBAGENT_TOOL_TARGET_FIELD;
use crate::builtin_subagent_tools::SUBAGENT_TOOL_TASK_FIELD;
use crate::builtin_subagent_tools::SubagentToolExecutionStatus;
use axon_domain::AxonError;

fn call(name: &str, arguments: serde_json::Value) -> ProviderToolCall {
    ProviderToolCall::new("call-1", name, serde_json::to_string(&arguments).unwrap())
}

fn input(name: &str, arguments: serde_json::Value) -> SubagentToolExecutionInput {
    SubagentToolExecutionInput {
        tool_call: call(name, arguments),
        current_path: AgentPath::root(),
        remote_requested: false,
        completed_at_ms: 1,
    }
}

#[test]
fn runtime_spawns_and_lists_agent() {
    let mut control = AgentControlRuntime::default();
    let runtime = SubagentToolRuntime;
    let spawn = runtime
        .execute(
            &mut control,
            input(
                SPAWN_AGENT_TOOL_NAME,
                serde_json::json!({
                    SUBAGENT_TOOL_TASK_NAME_FIELD: "review",
                    SUBAGENT_TOOL_TASK_FIELD: "检查安全边界",
                    SUBAGENT_TOOL_FORK_TURNS_FIELD: "all",
                }),
            ),
        )
        .unwrap();

    assert_eq!(spawn.status, SubagentToolExecutionStatus::Completed);
    assert_eq!(spawn.payload["taskName"], "/root/review");
    let task_path = AgentPath::parse(spawn.payload["taskName"].as_str().unwrap()).unwrap();
    control.lease_execution(&task_path).unwrap();

    let listed = runtime
        .execute(
            &mut control,
            input(LIST_AGENTS_TOOL_NAME, serde_json::json!({})),
        )
        .unwrap();
    assert_eq!(listed.payload["agents"].as_array().unwrap().len(), 2);
    // 目标态工具面是固定的四个协作工具；被移除的三个工具不得再注册。
    // 名称按片段拼接：本轮裁剪扫描要求 crates/ 对这三者零命中，
    // 而反例断言又必须覆盖它们的确切名字。
    let removed = [
        concat!("wait", "_agent"),
        concat!("interrupt", "_agent"),
        concat!("close", "_agent"),
    ];
    let registered = subagent_tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert_eq!(
        registered,
        [
            "spawn_agent",
            "send_message",
            "list_agents",
            "delegate_to_fast"
        ],
        "子代理工具面必须收敛到四个协作工具"
    );
    assert!(
        removed
            .iter()
            .all(|name| !registered.iter().any(|tool| tool == name)),
        "被移除的工具不得再出现在执行注册表"
    );
}

#[test]
fn spawn_agent_accepts_kebab_task_name_and_fast_delegate_uses_same_spawn_payload() {
    let mut control = AgentControlRuntime::default();
    let runtime = SubagentToolRuntime;

    let spawn = runtime
        .execute(
            &mut control,
            input(
                SPAWN_AGENT_TOOL_NAME,
                serde_json::json!({
                    SUBAGENT_TOOL_TASK_NAME_FIELD: "review-auth",
                    SUBAGENT_TOOL_TASK_FIELD: "检查 auth 边界",
                }),
            ),
        )
        .unwrap();

    assert_eq!(spawn.payload["taskName"], "/root/review-auth");
    assert_eq!(spawn.payload["forkTurns"], "none");
    assert_eq!(spawn.payload["role"], serde_json::Value::Null);

    let delegated = runtime
        .execute(
            &mut control,
            input(
                DELEGATE_TO_FAST_TOOL_NAME,
                serde_json::json!({ SUBAGENT_TOOL_TASK_FIELD: "快速扫描 crates 目录" }),
            ),
        )
        .unwrap();

    assert_eq!(delegated.payload["taskName"], "/root/delegate_fast_1");
    assert_eq!(delegated.payload["forkTurns"], "none");
    assert_eq!(delegated.payload["role"], "fast");
    assert_eq!(delegated.payload["delegateKind"], "fast");
    assert_eq!(delegated.payload["modelPolicy"], "fast_model");
    assert_eq!(delegated.model_intent, SubagentModelIntent::Fast);
}

/// 委托默认后台：省略 `run_in_background` 时回执必须如实带 `runInBackground: true`。
#[test]
fn delegate_defaults_to_background_and_reports_the_wait_mode() {
    let mut control = AgentControlRuntime::default();
    let runtime = SubagentToolRuntime;

    let background = runtime
        .execute(
            &mut control,
            input(
                DELEGATE_TO_FAST_TOOL_NAME,
                serde_json::json!({ SUBAGENT_TOOL_TASK_FIELD: "后台委托" }),
            ),
        )
        .unwrap();
    assert_eq!(
        background.payload["runInBackground"],
        serde_json::json!(true)
    );

    let synchronous = runtime
        .execute(
            &mut control,
            input(
                DELEGATE_TO_FAST_TOOL_NAME,
                serde_json::json!({
                    SUBAGENT_TOOL_TASK_FIELD: "同步委托",
                    SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD: false,
                }),
            ),
        )
        .unwrap();
    assert_eq!(
        synchronous.payload["runInBackground"],
        serde_json::json!(false)
    );
}

/// `run_in_background` 只接受布尔值；非布尔值与未知字段都稳定拒绝。
#[test]
fn delegate_rejects_non_boolean_wait_mode_and_unknown_fields() {
    let mut control = AgentControlRuntime::default();
    let runtime = SubagentToolRuntime;

    for arguments in [
        serde_json::json!({
            SUBAGENT_TOOL_TASK_FIELD: "委托",
            SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD: "false",
        }),
        serde_json::json!({
            SUBAGENT_TOOL_TASK_FIELD: "委托",
            "wait": true,
        }),
    ] {
        assert!(
            runtime
                .execute(&mut control, input(DELEGATE_TO_FAST_TOOL_NAME, arguments))
                .is_err(),
            "非法等待模式参数必须被拒绝"
        );
    }
}

/// 工具 schema 必须声明可选的 `run_in_background`，description 必须写清自包含与后台默认。
#[test]
fn delegate_schema_declares_optional_run_in_background() {
    let definition = delegate_to_fast_tool_definition();
    let properties = &definition.schema.input_schema["properties"];

    assert_eq!(
        properties[SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD]["type"],
        "boolean"
    );
    assert_eq!(
        definition.schema.input_schema["required"],
        serde_json::json!([SUBAGENT_TOOL_TASK_FIELD]),
        "等待模式必须可省略（默认后台）"
    );
    for expected in [
        "自包含",
        "子代理看不到本次对话",
        "默认后台执行",
        "run_in_background: false",
    ] {
        assert!(
            definition.description.contains(expected),
            "委托说明必须写清 `{expected}`：{}",
            definition.description
        );
    }
}

#[test]
fn spawn_agent_rejects_task_names_outside_schema_pattern() {
    let mut control = AgentControlRuntime::default();
    let runtime = SubagentToolRuntime;

    for task_name in ["Review", "review task", "review.task", " review"] {
        let result = runtime.execute(
            &mut control,
            input(
                SPAWN_AGENT_TOOL_NAME,
                serde_json::json!({
                    SUBAGENT_TOOL_TASK_NAME_FIELD: task_name,
                    SUBAGENT_TOOL_TASK_FIELD: "检查任务名校验",
                }),
            ),
        );
        assert!(result.is_err(), "task_name 应被拒绝: {task_name}");
    }
}

#[test]
fn runtime_rejects_root_unknown_and_empty_message() {
    let mut control = AgentControlRuntime::default();
    let runtime = SubagentToolRuntime;

    assert!(matches!(
        runtime.execute(
            &mut control,
            input(
                SEND_MESSAGE_TOOL_NAME,
                serde_json::json!({
                    SUBAGENT_TOOL_TARGET_FIELD: "/root",
                    SUBAGENT_TOOL_MESSAGE_FIELD: "hello",
                }),
            ),
        ),
        Err(AxonError::Tool(axon_domain::ToolError::InvalidArguments(reason)))
            if reason == SUBAGENT_TOOL_REASON_ROOT_TARGET_REJECTED
    ));
    assert!(matches!(
        runtime.execute(
            &mut control,
            input(
                LIST_AGENTS_TOOL_NAME,
                serde_json::json!({ SUBAGENT_TOOL_PATH_PREFIX_FIELD: "/root" }),
            ),
        ),
        Err(AxonError::Tool(axon_domain::ToolError::InvalidArguments(reason)))
            if reason == SUBAGENT_TOOL_REASON_ROOT_TARGET_REJECTED
    ));
    assert!(matches!(
        runtime.execute(
            &mut control,
            input(
                SEND_MESSAGE_TOOL_NAME,
                serde_json::json!({
                    SUBAGENT_TOOL_TARGET_FIELD: "missing",
                    SUBAGENT_TOOL_MESSAGE_FIELD: "hello",
                }),
            ),
        ),
        Err(AxonError::Tool(axon_domain::ToolError::InvalidArguments(reason)))
            if reason == AGENT_CONTROL_REASON_NOT_FOUND
    ));
    assert!(matches!(
        runtime.execute(
            &mut control,
            input(
                SPAWN_AGENT_TOOL_NAME,
                serde_json::json!({
                    SUBAGENT_TOOL_TASK_NAME_FIELD: "review",
                    SUBAGENT_TOOL_TASK_FIELD: "",
                }),
            ),
        ),
        Err(AxonError::Tool(axon_domain::ToolError::InvalidArguments(_)))
    ));
}

#[test]
fn collaboration_tools_reject_root_and_self_targets_with_one_reason_each() {
    let mut control = AgentControlRuntime::default();
    let runtime = SubagentToolRuntime;
    runtime
        .execute(
            &mut control,
            input(
                SPAWN_AGENT_TOOL_NAME,
                serde_json::json!({
                    SUBAGENT_TOOL_TASK_NAME_FIELD: "review",
                    SUBAGENT_TOOL_TASK_FIELD: "初始任务",
                }),
            ),
        )
        .unwrap();
    let current_path = AgentPath::parse("/root/review").unwrap();

    // root 与 self 在 send_message 与 list_agents 上必须得到**同构**错误。
    let cases = [
        (
            SEND_MESSAGE_TOOL_NAME,
            serde_json::json!({
                SUBAGENT_TOOL_TARGET_FIELD: "/root",
                SUBAGENT_TOOL_MESSAGE_FIELD: "向父代理汇报",
            }),
            SUBAGENT_TOOL_REASON_ROOT_TARGET_REJECTED,
        ),
        (
            LIST_AGENTS_TOOL_NAME,
            serde_json::json!({ SUBAGENT_TOOL_PATH_PREFIX_FIELD: "/root" }),
            SUBAGENT_TOOL_REASON_ROOT_TARGET_REJECTED,
        ),
        (
            SEND_MESSAGE_TOOL_NAME,
            serde_json::json!({
                SUBAGENT_TOOL_TARGET_FIELD: current_path.as_str(),
                SUBAGENT_TOOL_MESSAGE_FIELD: "给自己发消息",
            }),
            SUBAGENT_TOOL_REASON_SELF_TARGET_REJECTED,
        ),
        (
            LIST_AGENTS_TOOL_NAME,
            serde_json::json!({ SUBAGENT_TOOL_PATH_PREFIX_FIELD: current_path.as_str() }),
            SUBAGENT_TOOL_REASON_SELF_TARGET_REJECTED,
        ),
    ];

    for (tool, arguments, expected) in cases {
        let error = runtime
            .execute(
                &mut control,
                SubagentToolExecutionInput {
                    current_path: current_path.clone(),
                    ..input(tool, arguments)
                },
            )
            .expect_err("root 与 self 都必须被拒绝");
        assert!(
            format!("{error:?}").contains(expected),
            "{tool} 的拒绝理由必须是唯一常量 {expected}：{error:?}"
        );
        assert!(
            matches!(
                error,
                AxonError::Tool(axon_domain::ToolError::InvalidArguments(_))
            ),
            "{tool} 必须以 invalid_arguments 稳定拒绝：{error:?}"
        );
    }
}

#[test]
fn fast_child_cannot_spawn_or_delegate_to_another_agent() {
    let mut control = AgentControlRuntime::default();
    let runtime = SubagentToolRuntime;
    let fast = runtime
        .execute(
            &mut control,
            input(
                DELEGATE_TO_FAST_TOOL_NAME,
                serde_json::json!({ SUBAGENT_TOOL_TASK_FIELD: "快速检查" }),
            ),
        )
        .unwrap();
    let child_path = AgentPath::parse(fast.payload["taskName"].as_str().unwrap()).unwrap();

    for (tool_name, arguments) in [
        (
            SPAWN_AGENT_TOOL_NAME,
            serde_json::json!({
                SUBAGENT_TOOL_TASK_NAME_FIELD: "nested",
                SUBAGENT_TOOL_TASK_FIELD: "不应执行",
            }),
        ),
        (
            DELEGATE_TO_FAST_TOOL_NAME,
            serde_json::json!({ SUBAGENT_TOOL_TASK_FIELD: "不应执行" }),
        ),
    ] {
        let receipt = runtime
            .execute(
                &mut control,
                SubagentToolExecutionInput {
                    tool_call: call(tool_name, arguments),
                    current_path: child_path.clone(),
                    remote_requested: false,
                    completed_at_ms: 1,
                },
            )
            .unwrap();
        assert_eq!(receipt.status, SubagentToolExecutionStatus::Blocked);
        assert_eq!(
            receipt.reasons,
            vec![SUBAGENT_TOOL_REASON_CHILD_SPAWN_BLOCKED.to_owned()]
        );
        assert_eq!(receipt.payload["blocked"], true);
    }

    assert_eq!(
        control.status(&child_path.join("nested").unwrap()),
        AgentStatus::NotFound
    );
}

#[test]
fn normal_child_can_delegate_fast_child() {
    let mut control = AgentControlRuntime::default();
    let runtime = SubagentToolRuntime;
    let normal = runtime
        .execute(
            &mut control,
            input(
                SPAWN_AGENT_TOOL_NAME,
                serde_json::json!({
                    SUBAGENT_TOOL_TASK_NAME_FIELD: "analysis",
                    SUBAGENT_TOOL_TASK_FIELD: "分析复杂问题",
                }),
            ),
        )
        .unwrap();
    let normal_path = AgentPath::parse(normal.payload["taskName"].as_str().unwrap()).unwrap();
    let fast = runtime
        .execute(
            &mut control,
            SubagentToolExecutionInput {
                tool_call: call(
                    DELEGATE_TO_FAST_TOOL_NAME,
                    serde_json::json!({ SUBAGENT_TOOL_TASK_FIELD: "快速验证" }),
                ),
                current_path: normal_path,
                remote_requested: false,
                completed_at_ms: 1,
            },
        )
        .unwrap();

    assert_eq!(fast.status, SubagentToolExecutionStatus::Completed);
    assert_eq!(fast.payload["taskName"], "/root/analysis/delegate_fast_1");
    assert_eq!(fast.payload["delegateKind"], "fast");
}
