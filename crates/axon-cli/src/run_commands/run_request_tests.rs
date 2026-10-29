use super::super::run_request_subagents::subagent_graph_store;
use super::*;
use crate::stable_ids::stable_numeric_id;

#[test]
fn run_request_uses_configured_control_limits() {
    let workspace = axon_test_support::test_temp_root().join(format!(
        "axon-run-files-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos()
    ));
    std::fs::create_dir_all(&workspace).unwrap();
    let config_path = workspace.join("config.toml");
    let mut config = axon_config::AxonConfig::default();
    config.providers.push(axon_config::ProviderConfig::new(
        "fixture",
        "openai-chatcompletions",
        "https://provider.invalid/v1",
    ));
    config.models.push(axon_config::ModelConfig {
        name: "fixture-chat".to_owned(),
        id: "fixture-chat".to_owned(),
        provider: "fixture".to_owned(),
        context_window: 32_000,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: false,
        embedding_dimension: 0,
        requires_reasoning_content_on_assistant_messages: false,
    });
    config.agent.default_model = "fixture-chat".to_owned();
    config.agent.fast_model = "fixture-chat".to_owned();
    axon_config::ConfigLoader::save_to_path(&config_path, &config).unwrap();
    let config = axon_config::ConfigLoader::load_from_path(&config_path)
        .expect("配置应可读取")
        .config;

    let command = RunCommand {
        prompt: "解释错误".to_owned(),
        model: None,
        reasoning_effort: None,
        format: Some(OutputFormat::Json),
        continue_last: false,
        session: None,
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    };
    let startup = Arc::new(RunStartupSnapshot::build(&config_path, &workspace).unwrap());

    let request = RunRequest::from_command(
        &command,
        &config,
        &config_path,
        RunRequestSession::with_id(None, super::new_run_session_id()),
        Some(&workspace),
        RunRequestBuildOptions {
            client_channel: ClientChannel::Cli,
            startup,
            cancel_token: None,
            subagent_event_bus: None,
            subagent_session_sinks: None,
            agent_identity: RunAgentIdentity::PrimaryPrivate,
            performance: None,
            workspace_snapshot_prewarm: None,
        },
    )
    .expect("RunRequest 应可构造");

    let control_config = request.agent_control.lock().unwrap().config();
    assert_eq!(control_config.max_agents, config.agent.max_subagent_threads);
    assert_eq!(
        control_config.max_active_turns,
        config.agent.max_subagent_active_turns
    );

    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn goal_execution_subagent_runs_fast_model_without_main_turn_reasoning() {
    let workspace = axon_test_support::test_temp_root().join(format!(
        "axon-run-task-execution-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos()
    ));
    std::fs::create_dir_all(&workspace).unwrap();
    let config_path = workspace.join("config.toml");
    let mut config = axon_config::AxonConfig::default();
    config.providers.push(axon_config::ProviderConfig::new(
        "fixture",
        "openai-chatcompletions",
        "https://provider.invalid/v1",
    ));
    for model_id in ["main-model", "fast-model"] {
        config.models.push(axon_config::ModelConfig {
            name: model_id.to_owned(),
            id: model_id.to_owned(),
            provider: "fixture".to_owned(),
            context_window: 32_000,
            max_output_tokens: None,
            supports_vision: false,
            supports_embedding: false,
            embedding_dimension: 0,
            requires_reasoning_content_on_assistant_messages: false,
        });
    }
    config.agent.default_model = "main-model".to_owned();
    config.agent.fast_model = "fast-model".to_owned();
    config.agent.reasoning_effort = "high".to_owned();
    axon_config::ConfigLoader::save_to_path(&config_path, &config).unwrap();
    let config = axon_config::ConfigLoader::load_from_path(&config_path)
        .expect("配置应可读取")
        .config;

    let command = RunCommand {
        prompt: "执行已承诺的 Task".to_owned(),
        model: None,
        reasoning_effort: None,
        format: Some(OutputFormat::Json),
        continue_last: false,
        session: None,
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    };
    let startup = Arc::new(RunStartupSnapshot::build(&config_path, &workspace).unwrap());
    let build = |agent_identity: RunAgentIdentity| {
        RunRequest::from_command(
            &command,
            &config,
            &config_path,
            RunRequestSession::with_id(None, super::new_run_session_id()),
            Some(&workspace),
            RunRequestBuildOptions {
                client_channel: ClientChannel::Cli,
                startup: Arc::clone(&startup),
                cancel_token: None,
                subagent_event_bus: None,
                subagent_session_sinks: None,
                agent_identity,
                performance: None,
                workspace_snapshot_prewarm: None,
            },
        )
        .expect("RunRequest 应可构造")
    };

    let primary = build(RunAgentIdentity::PrimaryPrivate);
    let execution = build(RunAgentIdentity::GoalExecutionSubagent);

    assert_eq!(primary.model.id, "main-model");
    assert_eq!(primary.reasoning_effort.as_deref(), Some("high"));
    assert!(RunAgentIdentity::PrimaryPrivate.includes_personal_memory());
    assert!(RunAgentIdentity::PrimaryWorkspace.includes_user_soul());
    assert!(!RunAgentIdentity::PrimaryWorkspace.includes_personal_memory());
    assert_eq!(
        RunAgentIdentity::PrimaryWorkspace.memory_disclosure_policy(),
        axon_domain::DisclosurePolicy::Workspace
    );
    // 后台 Goal Execution 与 fast 子代理一致：fast 模型、不继承主 turn 思考强度、不加载 Soul。
    assert_eq!(execution.model.id, "fast-model");
    assert_eq!(execution.reasoning_effort, None);
    assert!(!RunAgentIdentity::GoalExecutionSubagent.includes_user_soul());
    assert_eq!(
        RunAgentIdentity::GoalExecutionSubagent.memory_disclosure_policy(),
        axon_domain::DisclosurePolicy::Workspace
    );
    let context_text = |request: &RunRequest| {
        request
            .model_payload
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(context_text(&primary).contains("<tool_orchestration_protocol>"));
    assert!(!context_text(&execution).contains("<tool_orchestration_protocol>"));

    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn image_attachment_is_not_inlined_into_prompt() {
    let workspace = axon_test_support::test_temp_root().join(format!(
        "axon-run-image-prompt-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos()
    ));
    std::fs::create_dir_all(&workspace).unwrap();
    let image_path = workspace.join("screen.png");
    std::fs::write(
        &image_path,
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
    )
    .unwrap();
    let command = RunCommand {
        prompt: "分析图片".to_owned(),
        model: None,
        reasoning_effort: None,
        format: Some(OutputFormat::Json),
        continue_last: false,
        session: None,
        file: vec![image_path.clone()],
        timeout: None,
        tool: Vec::new(),
    };

    let (prompt, attached_files) = compose_run_prompt(&command).expect("图片附件应可投影");

    assert_eq!(attached_files, vec![image_path.canonicalize().unwrap()]);
    assert_eq!(prompt, "分析图片");
    assert!(!prompt.contains(image_path.to_string_lossy().as_ref()));
    let _ = std::fs::remove_dir_all(workspace);
}

#[test]
fn text_attachment_is_not_inlined_and_can_be_the_only_user_input() {
    let workspace = axon_test_support::test_temp_root()
        .join(format!("axon-run-file-prompt-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    let file_path = workspace.join("report.txt");
    std::fs::write(&file_path, "不应进入 prompt 的附件正文").unwrap();
    let command = RunCommand {
        prompt: String::new(),
        model: None,
        reasoning_effort: None,
        format: Some(OutputFormat::Json),
        continue_last: false,
        session: None,
        file: vec![file_path.clone()],
        timeout: None,
        tool: Vec::new(),
    };

    let (prompt, attached_files) = compose_run_prompt(&command).expect("纯附件请求应有效");

    assert!(prompt.is_empty());
    assert_eq!(attached_files, vec![file_path.canonicalize().unwrap()]);
    let _ = std::fs::remove_dir_all(workspace);
}

#[test]
fn subagent_graph_store_uses_session_root_thread_id() {
    let workspace = axon_test_support::test_temp_root().join(format!(
        "axon-run-subagent-graph-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos()
    ));
    std::fs::create_dir_all(&workspace).unwrap();
    let config_path = workspace.join("isolated-config.toml");
    std::fs::write(&config_path, "").unwrap();
    let mut config = axon_config::AxonConfig::default();
    config.providers.push(axon_config::ProviderConfig::new(
        "fixture",
        "openai-chatcompletions",
        "https://provider.invalid/v1",
    ));
    config.models.push(axon_config::ModelConfig {
        name: "fixture-chat".to_owned(),
        id: "fixture-chat".to_owned(),
        provider: "fixture".to_owned(),
        context_window: 32_000,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: false,
        embedding_dimension: 0,
        requires_reasoning_content_on_assistant_messages: false,
    });
    config.agent.default_model = "fixture-chat".to_owned();
    config.agent.fast_model = "fixture-chat".to_owned();
    let command = RunCommand {
        prompt: "审查当前改动".to_owned(),
        model: None,
        reasoning_effort: None,
        format: Some(OutputFormat::Json),
        continue_last: false,
        session: None,
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    };
    let startup = Arc::new(RunStartupSnapshot::build(&config_path, &workspace).unwrap());
    let request = RunRequest::from_command(
        &command,
        &config,
        &config_path,
        RunRequestSession::with_id(None, super::new_run_session_id()),
        Some(&workspace),
        RunRequestBuildOptions {
            client_channel: ClientChannel::Cli,
            startup,
            cancel_token: None,
            subagent_event_bus: None,
            subagent_session_sinks: None,
            agent_identity: RunAgentIdentity::PrimaryPrivate,
            performance: None,
            workspace_snapshot_prewarm: None,
        },
    )
    .expect("RunRequest 应可构造");
    let root_thread_id = request
        .agent_control
        .lock()
        .unwrap()
        .root_thread_id()
        .unwrap();
    assert_eq!(root_thread_id, stable_numeric_id(&request.session_id));

    let store = request.agent_graph_store.clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        store
            .create_thread_node(
                axon_domain::ThreadSpawnNode::new(
                    root_thread_id,
                    "root".to_owned(),
                    workspace.to_string_lossy().to_string(),
                    0,
                    None,
                    1,
                )
                .with_agent_path("/root"),
            )
            .await
            .unwrap();
        store
            .create_thread_node(
                axon_domain::ThreadSpawnNode::new(
                    root_thread_id + 1,
                    "review".to_owned(),
                    workspace.to_string_lossy().to_string(),
                    1,
                    Some(root_thread_id),
                    1,
                )
                .with_agent_path("/root/review"),
            )
            .await
            .unwrap();
        store
            .upsert_thread_spawn_edge(root_thread_id, root_thread_id + 1, 1)
            .await
            .unwrap();
    });
    let graph_file = workspace.join("data").join("subagent-graph.json");
    assert!(graph_file.exists());

    let reopened = subagent_graph_store(&config_path);
    let child = rt
        .block_on(reopened.find_thread_spawn_child_by_path(root_thread_id, "/root/review"))
        .unwrap();
    assert_eq!(child.map(|node| node.id), Some(root_thread_id + 1));

    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn resumed_parent_session_rejects_self_reference() {
    assert_eq!(
        resumed_parent_session_id("session-1", Some("session-1")),
        None
    );
    assert_eq!(
        resumed_parent_session_id("session-1", Some("parent-1")).as_deref(),
        Some("parent-1")
    );
}
