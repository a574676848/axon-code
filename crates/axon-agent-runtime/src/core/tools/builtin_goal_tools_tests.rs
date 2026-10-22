//! Goal 工具「单一解析实现 + 五工具外部 schema 守恒」的回归测试。

use super::{
    GOAL_CANCEL_TOOL_NAME, GOAL_CREATE_TOOL_NAME, GOAL_GET_TOOL_NAME, GOAL_LIST_TOOL_NAME,
    GOAL_START_TOOL_NAME, GOAL_UPDATE_TOOL_NAME, GoalToolRuntime, goal_tool_definitions,
};
use axon_domain::{AxonError, ToolError};
use axon_protocol::GoalToolCommand;
use serde_json::json;

/// 构造模型面看到的限定工具名。
fn qualified(bare_name: &str) -> String {
    format!("builtin.{bare_name}")
}

/// 断言错误是「工具参数无效」这一类结构化拒绝。
fn assert_tool_invalid_arguments(error: AxonError, case: &str) {
    assert!(
        matches!(error, AxonError::Tool(ToolError::InvalidArguments(_))),
        "{case} 必须返回工具参数错误，实际为 {error:?}"
    );
}

/// 六个 Goal 工具必须各自映射到唯一的 wire 命令变体。
#[test]
fn every_goal_tool_maps_to_exactly_one_command_variant() {
    let cases = [
        (
            GOAL_CREATE_TOOL_NAME,
            json!({
                "scope": "current_workspace",
                "title": "整理发布说明",
                "objective": "输出可验收的发布说明",
                "steps": ["汇总变更"],
                "acceptance": ["发布说明通过审阅"],
            }),
        ),
        (GOAL_LIST_TOOL_NAME, json!({"scope": "current_context"})),
        (GOAL_GET_TOOL_NAME, json!({"goalId": "goal-1"})),
        (
            GOAL_UPDATE_TOOL_NAME,
            json!({"goalId": "goal-1", "expectedRevision": 3, "nextStep": "复核产物"}),
        ),
        (
            GOAL_CANCEL_TOOL_NAME,
            json!({"goalId": "goal-1", "expectedRevision": 3, "reason": "用户取消"}),
        ),
        (
            GOAL_START_TOOL_NAME,
            json!({"goalId": "goal-1", "expectedRevision": 3}),
        ),
    ];
    let mut variants = Vec::new();
    for (bare_name, input) in cases {
        let name = qualified(bare_name);
        let command = GoalToolRuntime::parse_command(&name, &input)
            .unwrap_or_else(|error| panic!("{name} 必须解析成功：{error:?}"));
        variants.push(command_variant(&command));
        assert_eq!(
            command_variant(&command),
            bare_name,
            "{name} 命令变体必须唯一"
        );
    }
    variants.sort_unstable();
    variants.dedup();
    assert_eq!(variants.len(), 6, "六个工具的 wire 命令变体必须两两不同");
}

/// 缺省值由唯一解析实现补齐，下游层不再二次计算。
#[test]
fn the_single_parser_applies_documented_defaults() {
    let list = GoalToolRuntime::parse_command(
        &qualified(GOAL_LIST_TOOL_NAME),
        &json!({
            "scope": "current_context"
        }),
    )
    .expect("列表命令解析成功");
    assert!(
        matches!(list, GoalToolCommand::List(query) if query.limit == 20),
        "列表缺省页大小必须为 20"
    );

    let get = GoalToolRuntime::parse_command(
        &qualified(GOAL_GET_TOOL_NAME),
        &json!({
            "goalId": "goal-1"
        }),
    )
    .expect("详情命令解析成功");
    assert!(
        matches!(get, GoalToolCommand::Get(query) if query.limit == 20),
        "详情缺省记录上限必须为 20"
    );

    let create = GoalToolRuntime::parse_command(
        &qualified(GOAL_CREATE_TOOL_NAME),
        &json!({
            "scope": "global",
            "title": "标题",
            "objective": "目标",
            "steps": ["步骤"],
            "acceptance": ["验收"],
        }),
    )
    .expect("创建命令解析成功");
    assert!(
        matches!(create, GoalToolCommand::Create(command) if !command.create_execution),
        "缺省不创建首个 Execution"
    );
}

/// 未知工具名、未限定名与非法参数统一返回结构化工具参数拒绝。
#[test]
fn unknown_tool_names_and_invalid_arguments_are_rejected() {
    let valid_create = json!({
        "scope": "global",
        "title": "标题",
        "objective": "目标",
        "steps": ["步骤"],
        "acceptance": ["验收"],
    });
    let cases: Vec<(String, serde_json::Value)> = vec![
        (qualified("goal_unknown"), json!({})),
        (qualified("goal_archive"), json!({})),
        ("goal_create".to_owned(), valid_create.clone()),
        (qualified(GOAL_CREATE_TOOL_NAME), {
            let mut input = valid_create.clone();
            input["scope"] = json!("unsupported_scope");
            input
        }),
        (qualified(GOAL_CREATE_TOOL_NAME), {
            let mut input = valid_create.clone();
            input["title"] = json!("   ");
            input
        }),
        (qualified(GOAL_CREATE_TOOL_NAME), {
            let mut input = valid_create.clone();
            input["createExecutionAlias"] = json!(true);
            input
        }),
        (
            qualified(GOAL_LIST_TOOL_NAME),
            json!({"scope": "current_context", "limit": 0}),
        ),
        (
            qualified(GOAL_LIST_TOOL_NAME),
            json!({"scope": "unsupported_scope"}),
        ),
        (
            qualified(GOAL_UPDATE_TOOL_NAME),
            json!({"goalId": "goal-1", "expectedRevision": 1}),
        ),
        (
            qualified(GOAL_CANCEL_TOOL_NAME),
            json!({"goalId": "goal-1", "expectedRevision": 1, "reason": "  "}),
        ),
    ];
    for (name, input) in cases {
        let case = format!("{name} + {input}");
        let error = GoalToolRuntime::parse_command(&name, &input)
            .expect_err("该输入必须被单一解析实现拒绝");
        assert_tool_invalid_arguments(error, &case);
    }
}

/// §5.4.3 守恒项：五工具的外部 schema 与目标态逐字段一致，无别名无兼容层。
#[test]
fn goal_tool_definitions_freeze_the_six_external_schemas() {
    let definitions = goal_tool_definitions();
    let frozen: [(&str, &[&str], &[&str]); 6] = [
        (
            GOAL_CREATE_TOOL_NAME,
            &["scope", "title", "objective", "steps", "acceptance"],
            &[
                "acceptance",
                "createExecution",
                "objective",
                "scope",
                "steps",
                "title",
            ],
        ),
        (
            GOAL_LIST_TOOL_NAME,
            &["scope"],
            &["cursor", "limit", "scope", "status"],
        ),
        (
            GOAL_GET_TOOL_NAME,
            &["goalId"],
            &["goalId", "includeEvents", "includeExecutions", "limit"],
        ),
        (
            GOAL_UPDATE_TOOL_NAME,
            &["goalId", "expectedRevision"],
            &[
                "expectedRevision",
                "goalId",
                "nextStep",
                "priority",
                "status",
                "title",
                "waitingReason",
            ],
        ),
        (
            GOAL_CANCEL_TOOL_NAME,
            &["goalId", "expectedRevision", "reason"],
            &["expectedRevision", "goalId", "reason"],
        ),
        (
            GOAL_START_TOOL_NAME,
            &["goalId", "expectedRevision"],
            &["expectedRevision", "goalId"],
        ),
    ];
    assert_eq!(
        definitions.len(),
        frozen.len(),
        "外部 Goal 工具必须恰为六个"
    );
    for (definition, (name, required, properties)) in definitions.iter().zip(frozen) {
        assert_eq!(definition.name, name, "工具名与顺序必须保持目标态");
        assert_eq!(definition.namespace, "builtin");
        assert!(definition.enabled, "{name} 必须处于启用状态");
        assert_eq!(
            definition.schema.required_field_names(),
            required,
            "{name} 必填字段发生变化"
        );
        let mut actual = definition.schema.field_names();
        actual.sort_unstable();
        assert_eq!(actual, properties, "{name} 属性集合发生变化");
        assert_eq!(
            definition.schema.input_schema["additionalProperties"],
            json!(false),
            "{name} 不得放开额外字段（即不得引入别名或兼容层）"
        );
        assert_eq!(
            definition.schema.input_schema["type"],
            json!("object"),
            "{name} 必须保持 object 入参"
        );
    }
}

/// 从 wire 命令取回其工具名，用于断言「一命令一变体」。
fn command_variant(command: &GoalToolCommand) -> &'static str {
    match command {
        GoalToolCommand::Create(_) => GOAL_CREATE_TOOL_NAME,
        GoalToolCommand::List(_) => GOAL_LIST_TOOL_NAME,
        GoalToolCommand::Get(_) => GOAL_GET_TOOL_NAME,
        GoalToolCommand::Update(_) => GOAL_UPDATE_TOOL_NAME,
        GoalToolCommand::Cancel(_) => GOAL_CANCEL_TOOL_NAME,
        GoalToolCommand::Start(_) => GOAL_START_TOOL_NAME,
    }
}
