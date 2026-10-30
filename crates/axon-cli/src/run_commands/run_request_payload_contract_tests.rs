use super::*;

const TEST_CONTEXT_WINDOW: u64 = 32_000;
const TEST_PROMPT: &str = "分析当前 Workspace";
const TEST_SESSION_ID: &str = "workspace-prewarm-contract";
const ENVIRONMENT_CONTEXT_PREFIX: &str = "<environment-context>";
const NORMALIZED_ENVIRONMENT_CONTEXT: &str =
    "<environment-context>normalized</environment-context>";

#[test]
fn workspace_snapshot_prewarm_matches_sync_payload_and_consumes_once() {
    let root = axon_test_support::test_temp_dir();
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&home).expect("创建 Home 根目录");
    std::fs::create_dir_all(&workspace).expect("创建 Workspace");
    initialize_git_workspace(&workspace);
    let config_path = home.join("config.toml");
    let config = AxonConfig::default();
    axon_config::ConfigLoader::save_to_path(&config_path, &config).expect("写入测试配置");
    let startup = RunStartupSnapshot::build(&config_path, &workspace).expect("构造启动快照");
    let command = contract_command();

    let mut synchronous_snapshot = resolve_run_workspace_snapshot(&config_path, &workspace)
        .expect("同步解析成功")
        .expect("同步路径应解析 Workspace");
    let prewarm = RunWorkspaceSnapshotPrewarm::spawn(config_path.clone(), workspace.clone(), None);
    let mut prewarmed_snapshot = prewarm
        .resolve()
        .expect("预热解析成功")
        .expect("预热路径应解析 Workspace");

    synchronous_snapshot.workspace.last_seen_at_ms = 0;
    prewarmed_snapshot.workspace.last_seen_at_ms = 0;
    assert_eq!(prewarmed_snapshot, synchronous_snapshot);
    assert_eq!(
        memory_scope(Some(&prewarmed_snapshot)),
        memory_scope(Some(&synchronous_snapshot))
    );
    assert!(prewarm.resolve().is_err(), "同一预热结果不得重复消费");

    let synchronous = build_contract_payload(
        &config_path,
        &config,
        &workspace,
        &command,
        &startup,
        None,
        false,
    );
    let prewarmed = build_contract_payload(
        &config_path,
        &config,
        &workspace,
        &command,
        &startup,
        Some(RunWorkspaceSnapshotPrewarm::spawn(
            config_path.clone(),
            workspace.clone(),
            None,
        )),
        false,
    );

    assert_eq!(
        normalize_environment_context(prewarmed.model_payload),
        normalize_environment_context(synchronous.model_payload)
    );
    assert_eq!(prewarmed.skill_selection, synchronous.skill_selection);
    assert_eq!(prewarmed.memory_recall, synchronous.memory_recall);
}

fn build_contract_payload(
    config_path: &Path,
    config: &AxonConfig,
    workspace: &Path,
    command: &RunCommand,
    startup: &RunStartupSnapshot,
    workspace_snapshot_prewarm: Option<RunWorkspaceSnapshotPrewarm>,
    include_user_soul: bool,
) -> BuildRunModelPayloadOutput {
    let input = BuildRunModelPayloadInput {
        transport_context: None,
        notification_chat_id: None,
        config_path,
        client_channel: axon_client_identity::ClientChannel::Cli,
        config,
        cwd: workspace,
        command,
        startup,
        stable_prefix: &startup.prompt_context.stable_prefix,
        prompt_for_model: TEST_PROMPT,
        session_id: TEST_SESSION_ID,
        model_history_json: None,
        attached_files: &[],
        supports_vision: false,
        context_window: TEST_CONTEXT_WINDOW,
        include_user_soul,
        include_personal_memory: false,
        memory_disclosure_policy: DisclosurePolicy::PrivateOnly,
        performance: None,
        workspace_snapshot_prewarm,
    };
    build_run_model_payload(&input, &ToolRegistry::new()).expect("构造合同 payload")
}

fn normalize_environment_context(mut payload: AgentModelPayload) -> AgentModelPayload {
    normalize_environment_messages(&mut payload.messages);
    normalize_environment_messages(&mut payload.injected_context_messages);
    payload
}

fn normalize_environment_messages(messages: &mut [AgentModelMessage]) {
    for message in messages {
        if message.content.starts_with(ENVIRONMENT_CONTEXT_PREFIX) {
            message.content = NORMALIZED_ENVIRONMENT_CONTEXT.to_owned();
        }
    }
}

fn initialize_git_workspace(workspace: &Path) {
    let status = axon_process::non_interactive_command("git")
        .args(["init", "-q"])
        .current_dir(workspace)
        .status()
        .expect("Git 可执行");
    assert!(status.success(), "初始化 Git Workspace 失败");
}

fn contract_command() -> RunCommand {
    RunCommand {
        prompt: TEST_PROMPT.to_owned(),
        model: None,
        reasoning_effort: None,
        format: None,
        continue_last: false,
        session: None,
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    }
}

#[test]
fn persona_segments_follow_execution_identity() {
    let root = axon_test_support::test_temp_dir();
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&home).expect("创建 Home 根目录");
    std::fs::create_dir_all(&workspace).expect("创建 Workspace");
    initialize_git_workspace(&workspace);
    let config_path = home.join("config.toml");
    let config = AxonConfig::default();
    axon_config::ConfigLoader::save_to_path(&config_path, &config).expect("写入测试配置");
    std::fs::write(
        home.join("SOUL.md"),
        "---\nschema: axon-soul/v1\nidentity:\n  name: Axon\n  address: 长官\n  role: 副官\nbackstory: 来处。\ninvariants:\n  - 依据事实作答\ntraits: [沉稳]\nvoice:\n  warmth: 40\n  formality: 60\n  humor: subtle\n  verbosity: concise\n---\n",
    )
    .expect("写入人设");
    let startup = RunStartupSnapshot::build(&config_path, &workspace).expect("构造启动快照");
    let command = contract_command();

    let primary = build_contract_payload(
        &config_path,
        &config,
        &workspace,
        &command,
        &startup,
        None,
        true,
    );
    let subagent = build_contract_payload(
        &config_path,
        &config,
        &workspace,
        &command,
        &startup,
        None,
        false,
    );

    let primary_text = format!("{:?}", primary.model_payload);
    let subagent_text = format!("{:?}", subagent.model_payload);
    assert!(
        primary_text.contains("<assistant-profile-context>"),
        "主代理必须装配人格内核"
    );
    assert!(
        subagent_text.is_empty() || !subagent_text.contains("<assistant-profile-context>"),
        "子代理不得继承人格内核"
    );
    assert!(
        !subagent_text.contains("<persona-state-context>"),
        "子代理不得继承人格状态段"
    );
}
