//! 子代理 child_request 构造测试：身份隔离、运行上下文继承与结构化输出。

use super::super::child_persistence::SUBAGENT_RESULT_INLINE_CHAR_LIMIT;
use super::*;

#[test]
fn child_request_overrides_system_prompt_without_tools() {
    let config_path = temp_config_path("planner-context");
    let executor = ProviderRunSubagentExecutor::new(fixture_environment(config_path.clone()));
    let mut input = fixture_execution_input("/root/planner");
    input.message = "生成执行计划".to_owned();
    input.system_prompt_override = Some("固定 Planner system prompt".to_owned());
    input.tools_enabled = false;

    let child_request = executor
        .child_request(&input, None)
        .expect("子代理应复用普通 coding 子请求");

    assert!(child_request.model_payload.model_tools.is_empty());
    assert!(
        child_request
            .execution_tools
            .read()
            .unwrap()
            .model_tools()
            .is_empty()
    );
    let messages = &child_request.model_payload.messages;
    let system_message = messages
        .iter()
        .find(|message| message.role == "system")
        .expect("子代理应保留普通 coding system 消息位置");
    assert!(
        system_message
            .content
            .starts_with("固定 Planner system prompt\n\n")
    );
    assert!(system_message.content.contains(SUBAGENT_RESULT_MARKER_TEXT));
    assert!(!system_message.cache_excluded);
    assert!(
        messages
            .iter()
            .any(|message| message.role == "user" && message.content == "生成执行计划")
    );
    let _ = fs::remove_dir_all(config_path.parent().unwrap());
}

#[test]
fn structured_child_request_omits_result_instruction_and_preserves_schema() {
    let config_path = temp_config_path("structured-output");
    let executor = ProviderRunSubagentExecutor::new(fixture_environment(config_path.clone()));
    let mut input = fixture_execution_input("/root/semantic-assessor");
    input.message = "评估本轮语义显著性".to_owned();
    input.system_prompt_override = Some("仅输出符合 schema 的 JSON".to_owned());
    input.structured_output = Some(ProviderStructuredOutputContract {
        schema: serde_json::json!({
            "type": "object",
            "required": ["schemaVersion", "shouldReflect"]
        }),
        mode: ProviderStructuredOutputMode::JsonSchema,
    });
    input.tools_enabled = false;

    let child_request = executor
        .child_request(&input, None)
        .expect("结构化子代理应复用普通 coding 子请求");

    assert_eq!(child_request.structured_output, input.structured_output);
    let system_message = child_request
        .model_payload
        .messages
        .iter()
        .find(|message| message.role == "system")
        .expect("结构化子代理应保留 system 消息");
    assert_eq!(system_message.content, "仅输出符合 schema 的 JSON");
    assert!(!system_message.content.contains(SUBAGENT_RESULT_MARKER_TEXT));
    let _ = fs::remove_dir_all(config_path.parent().unwrap());
}

#[test]
fn provider_child_request_inherits_parent_runtime_context() {
    let root = temp_config_path("child-request")
        .parent()
        .expect("临时配置路径应有父目录")
        .to_path_buf();
    let config_path = root.join("home").join("config.toml");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("AGENTS.md"), "子代理项目规则").unwrap();
    let skill_dir = workspace.join(".axon").join("skills").join("review");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: 审查代码\n---\n审查实现",
    )
    .unwrap();
    let mut environment = fixture_environment(config_path.clone());
    environment.cwd = workspace;
    environment.startup = Arc::new(
        crate::run_commands::RunStartupSnapshot::build(&config_path, &environment.cwd).unwrap(),
    );
    environment.timeout = Duration::from_secs(3);
    let parent_cancel = RunCancelToken::default();
    environment.cancel_token = Some(parent_cancel.clone());

    let executor = ProviderRunSubagentExecutor::new(environment.clone());
    let input = RunSubagentExecutionInput {
        task_path: AgentPath::parse("/root/review").unwrap(),
        message: "审查当前改动".to_owned(),
        fork_context: SubagentForkContextRuntime
            .plan(SubagentForkContextInput {
                parent_messages: vec![
                AgentModelMessage::text("system", "父上下文"),
                AgentModelMessage::text(
                    "developer",
                    "# AGENTS.md instructions for old\n\n<INSTRUCTIONS>\n旧规则\n</INSTRUCTIONS>",
                ),
                AgentModelMessage::text("user", "父级历史问题"),
                ],
                fork_turns: SubagentForkTurns::All,
                parent_usage_hint_texts: Vec::new(),
                child_usage_hint_text: None,
                usage_hint_enabled: false,
            })
            .unwrap(),
        started_at_ms: 1,
        max_runtime_ms: Some(2_000),
        model_intent: RunSubagentModelIntent::Inherit,
        structured_output: None,
        system_prompt_override: None,
        tools_enabled: true,
    };

    let child_request = executor
        .child_request(&input, Some(parent_cancel.child_token()))
        .expect("继承模型应能生成子代理请求");

    assert_eq!(
        child_request.session_id,
        "parent-session--subagent--root_review"
    );
    assert_eq!(child_request.config_path, config_path);
    assert_eq!(child_request.cwd, environment.cwd);
    assert_eq!(child_request.prompt, "审查当前改动");
    assert_eq!(child_request.routing_key, environment.routing_key);
    assert_eq!(child_request.timeout, Duration::from_secs(2));
    assert_eq!(child_request.model_profile_id, ModelProfileId(1));
    assert_eq!(
        child_request.permission_snapshot_id,
        PermissionSnapshotId(9)
    );
    assert_eq!(
        child_request.current_agent_path,
        AgentPath::parse("/root/review").unwrap()
    );
    assert!(
        !child_request
            .model_payload
            .model_tools
            .iter()
            .any(|tool| tool.qualified_name == "builtin.spawn_agent")
    );
    assert!(
        !child_request
            .model_payload
            .model_tools
            .iter()
            .any(|tool| tool.qualified_name == "builtin.update_plan"),
        "计划权只属于主 turn，子代理 payload 必须过滤 update_plan"
    );
    assert!(
        !child_request
            .model_payload
            .model_tools
            .iter()
            .any(|tool| axon_agent_runtime::GoalToolRuntime::is_goal_tool(&tool.qualified_name)),
        "Goal 账本只属于主 turn，子代理 payload 必须过滤 builtin.goal_*"
    );
    // PAR-1 §8.2：委派与控制类工具只在主代理面。
    for hidden in [
        "builtin.delegate_to_fast",
        "builtin.send_message",
        "builtin.list_agents",
    ] {
        assert!(
            !child_request
                .model_payload
                .model_tools
                .iter()
                .any(|tool| tool.qualified_name == hidden),
            "子代理 payload 必须过滤 {hidden}"
        );
    }
    assert!(
        child_request
            .model_payload
            .messages
            .iter()
            .any(|message| message.role == "system" && message.content.contains("父上下文"))
    );
    assert!(child_request.model_payload.messages.iter().any(|message| {
        message.role == "system"
            && message.content.contains(SUBAGENT_RESULT_MARKER_TEXT)
            && message
                .content
                .contains(&SUBAGENT_RESULT_INLINE_CHAR_LIMIT.to_string())
    }));
    assert!(
        child_request
            .model_payload
            .messages
            .iter()
            .any(|message| message.role == "developer"
                && message.content.contains("# AGENTS.md instructions")
                && message.content.contains("子代理项目规则"))
    );
    assert!(child_request.model_payload.messages.iter().any(|message| {
        message.role == "developer"
            && message.content.contains("<available-skills-context>")
            && message.content.contains("name: review")
    }));
    assert!(
        child_request
            .model_payload
            .messages
            .iter()
            .all(|message| !message.content.contains("inherited=\"parent\""))
    );
    assert!(
        child_request
            .model_payload
            .messages
            .iter()
            .all(|message| !message.content.contains("旧规则"))
    );
    assert!(child_request.selected_skill_name.is_none());
    assert!(
        child_request
            .model_payload
            .messages
            .iter()
            .all(
                |message| !message.content.contains("<tool_orchestration_protocol>")
                    && !message.content.contains("builtin.update_plan")
                    && !message.content.contains("builtin.delegate_to_fast")
            ),
        "子代理上下文不得携带主 turn 工具策略"
    );
    assert!(
        child_request
            .model_payload
            .messages
            .iter()
            .any(|message| message.content.contains("<delegation-policy>")),
        "子代理仍应保留委托机制契约"
    );
    let workspace_index = child_request
        .model_payload
        .messages
        .iter()
        .position(|message| message.content.contains("# AGENTS.md instructions"))
        .expect("子代理应复用 Workspace 动态上下文");
    let skills_index = child_request
        .model_payload
        .messages
        .iter()
        .position(|message| message.content.contains("<available-skills-context>"))
        .expect("子代理应复用 available Skills 动态上下文");
    let parent_history_index = child_request
        .model_payload
        .messages
        .iter()
        .position(|message| message.content == "父级历史问题")
        .expect("子代理应保留父级历史");
    assert!(parent_history_index < workspace_index);
    assert!(workspace_index < skills_index);
    let child_prompt = child_request.model_payload.messages.last().unwrap();
    assert_eq!(child_prompt.role, "user");
    assert_eq!(child_prompt.content, "审查当前改动",);
    assert!(Arc::ptr_eq(
        &child_request.agent_control,
        &environment.agent_control
    ));
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&child_request.agent_graph_store),
        Arc::as_ptr(&environment.agent_graph_store)
    ));
    assert!(Arc::ptr_eq(
        &child_request.subagent_sessions,
        &environment.subagent_sessions
    ));
    assert!(Arc::ptr_eq(
        &child_request.subagent_cancel_tokens,
        &environment.subagent_cancel_tokens
    ));
    parent_cancel.cancel();
    assert!(child_request.cancel_token.unwrap().is_cancelled());
    let _ = fs::remove_dir_all(root);
}
