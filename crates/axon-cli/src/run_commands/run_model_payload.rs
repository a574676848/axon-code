//! `axon run` 模型 payload 构造。

use super::RunStartupSnapshot;
use crate::{
    run_commands::run_workspace_identity::workspace_git_state,
    runtime_capability_routing::CapabilityRoutingFragment,
    runtime_prompt_fragments::assemble_cross_workspace_stable_context_messages,
    runtime_skill_context_fragments::SelectedSkillDeveloperFragment,
    runtime_workspace_instructions::WorkspaceInstruction,
};
use axon_agent_runtime::{
    AgentMemoryRecallInput, AgentModelMessage, AgentModelPayload, AgentModelStablePrefix,
    AgentPaneProjection, AgentPaneProjectionInput, ContextAssemblerConfig, ContextAssemblerInput,
    ContextAssemblerRuntime, LedgerContextProjection, MemoryHybridHit, ToolRegistry,
    agent_model_runtime_constraints, model_context_snapshot_budgets,
};
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, PaneId, PaneSecurityBadge,
    PaneSecurityBadgeLevel, PermissionSnapshotId,
};
use axon_session::WorkspaceSnapshot;
use chrono::Local;
use std::time::{SystemTime, UNIX_EPOCH};

const HOME_WITHOUT_WORKSPACE: &str = "未选择（当前目录位于 Home）";

#[derive(Debug, Clone, Copy)]
pub(super) struct CliRunModelPayloadInput<'a> {
    pub(super) memory_hits: &'a [MemoryHybridHit],
    pub(super) soul_context_message: Option<&'a AgentModelMessage>,
    /// 本轮人格状态动态段；不进缓存，计入动态尾部。
    pub(super) persona_state_message: Option<&'a AgentModelMessage>,
    pub(super) include_personal_memory: bool,
    pub(super) memory_disclosure_policy: DisclosurePolicy,
    pub(super) cwd: &'a str,
    pub(super) workspace_snapshot: Option<&'a WorkspaceSnapshot>,
    pub(super) startup: &'a RunStartupSnapshot,
    /// 本次请求使用的稳定前缀：主 turn 与子代理身份由调用方显式选择。
    pub(super) stable_prefix: &'a AgentModelStablePrefix,
    pub(super) context_window: u64,
    pub(super) max_subagent_threads: usize,
    pub(super) max_subagent_active_turns: usize,
    pub(super) model_history_json: Option<&'a str>,
    pub(super) ledger_context: Option<&'a LedgerContextProjection>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CliRunSkillSelection {
    pub(super) name: Option<String>,
    pub(super) source_ref: Option<String>,
    pub(super) prompt: Option<String>,
}

pub(super) struct CliRunSkillContext {
    pub(super) prompt: String,
    pub(super) selection: CliRunSkillSelection,
    pub(super) relevant_skills: String,
}

pub(super) fn prepare_cli_run_skill_context(
    startup: &RunStartupSnapshot,
    prompt: &str,
    context_window: u64,
) -> CliRunSkillContext {
    let skill_report = startup.skills.select_for_prompt(prompt);
    let selection = CliRunSkillSelection {
        name: skill_report.selected_skill_name().map(ToOwned::to_owned),
        source_ref: skill_report
            .selected
            .as_ref()
            .map(|record| record.source_ref.clone()),
        prompt: skill_report.selected_skill_prompt,
    };
    CliRunSkillContext {
        prompt: skill_report.user_prompt,
        selection,
        relevant_skills: startup.skills.render_relevant(prompt, context_window),
    }
}

pub(super) fn build_cli_run_model_payload(
    input: CliRunModelPayloadInput<'_>,
    skill_context: CliRunSkillContext,
    registry: &ToolRegistry,
) -> Result<(AgentModelPayload, CliRunSkillSelection), String> {
    let CliRunSkillContext {
        prompt,
        selection: skill_selection,
        relevant_skills,
    } = skill_context;
    let pane_id = PaneId(0);
    let (total_limit, reserved_for_tools) = model_context_snapshot_budgets(input.context_window)
        .map_err(|error| format!("模型上下文窗口无效：{error:?}"))?;
    let runtime_constraints = cli_runtime_constraints(
        skill_selection.name.as_deref(),
        input.startup.home_root.as_str(),
        input.startup.workspace_root.as_deref(),
        &input.startup.workspace_instructions,
        input.memory_hits,
        input.max_subagent_threads,
        input.max_subagent_active_turns,
    );
    let prompt_fragments = cli_dynamic_prompt_fragments(&skill_selection, &relevant_skills);
    let history_messages = model_history_messages(input.model_history_json)?;
    let git_state = workspace_git_state(input.workspace_snapshot);
    let stable_context_messages = assemble_cross_workspace_stable_context_messages(
        input.soul_context_message,
        input.startup.personal_memory.as_ref(),
        input.include_personal_memory,
        input.startup.user_agents.as_ref(),
    );
    let mut dynamic_context_messages = input
        .startup
        .prompt_context
        .workspace_context_messages
        .clone();
    if let Some(persona) = input.persona_state_message {
        dynamic_context_messages.push(persona.clone());
    }
    let environment_context = cli_environment_context();
    let payload = ContextAssemblerRuntime
        .assemble_input(ContextAssemblerInput {
            config: ContextAssemblerConfig {
                selected_skill_name: skill_selection.name.clone(),
                workspace_instruction_paths: input
                    .startup
                    .workspace_instructions
                    .iter()
                    .map(|instruction| instruction.path.clone())
                    .collect(),
            },
            snapshot_input: axon_agent_runtime::AgentContextSnapshotInput {
                pane_id,
                cwd: input.cwd.to_owned(),
                git_state: git_state.clone(),
                shell_history: Vec::new(),
                command_activity: None,
                apply_history: Vec::new(),
                chat_history: Vec::new(),
                total_limit,
                reserved_for_tools,
            },
            memory_recall_input: Some(AgentMemoryRecallInput {
                hits: input.memory_hits.to_vec(),
                code_symbol_hits: Vec::new(),
                audience: AudienceContext::new(
                    AudienceKind::Private,
                    input.memory_disclosure_policy,
                ),
            }),
            ledger_context: input.ledger_context.cloned(),
            stable_prefix: input.stable_prefix.clone(),
            stable_context_messages,
            dynamic_context_messages,
            environment_context: Some(environment_context),
            history_messages,
            repo_structure: None,
            user_prompt: prompt,
            runtime_constraints: Some(runtime_constraints),
            pane_projection: cli_pane_projection(pane_id, input.cwd, git_state),
            additional_fragments: prompt_fragments,
        })
        .map_err(|error| format!("模型请求 payload 构造失败：{error:?}"))?;
    let mut model_payload = payload.payload;
    model_payload.model_tools = registry.model_tools_for_provider();
    Ok((model_payload, skill_selection))
}

fn cli_environment_context() -> axon_agent_runtime::EnvironmentContext {
    let now = SystemTime::now();
    let utc_now_ms = now.duration_since(UNIX_EPOCH).map_or(0, |duration| {
        duration.as_millis().try_into().unwrap_or(u64::MAX)
    });
    let local_now: chrono::DateTime<Local> = now.into();
    let timezone_name = local_now.format("%Z").to_string();
    axon_agent_runtime::EnvironmentContext {
        utc_now_ms,
        local_time: Some(local_now.format("%Y-%m-%d %H:%M:%S").to_string()),
        timezone_name: (!timezone_name.trim().is_empty()).then_some(timezone_name),
        local_offset_minutes: Some(local_now.offset().local_minus_utc() / 60),
        platform: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        shell: Some(axon_agent_runtime::default_shell_program()),
        terminal: first_environment_value(&["TERM_PROGRAM", "TERM"]),
    }
}

fn first_environment_value(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn cli_dynamic_prompt_fragments(
    skill_selection: &CliRunSkillSelection,
    relevant_skills: &str,
) -> Vec<Box<dyn axon_agent_runtime::PromptFragment>> {
    let mut fragments: Vec<Box<dyn axon_agent_runtime::PromptFragment>> = Vec::new();
    if let Some(fragment) = skill_selection
        .prompt
        .clone()
        .and_then(SelectedSkillDeveloperFragment::optional)
    {
        fragments.push(Box::new(fragment));
    }
    if let Some(fragment) = CapabilityRoutingFragment::from_runtime(relevant_skills) {
        fragments.push(Box::new(fragment));
    }
    fragments
}

fn model_history_messages(history_json: Option<&str>) -> Result<Vec<AgentModelMessage>, String> {
    history_json.map_or_else(
        || Ok(Vec::new()),
        |history_json| {
            serde_json::from_str(history_json).map_err(|error| format!("模型历史格式无效：{error}"))
        },
    )
}

fn cli_pane_projection(
    pane_id: PaneId,
    cwd: &str,
    git_state: Option<axon_agent_runtime::AgentPaneGitState>,
) -> AgentPaneProjection {
    AgentPaneProjection::from_input(AgentPaneProjectionInput {
        pane_id,
        cwd: cwd.to_owned(),
        shell_kind: "cli".to_owned(),
        git_state,
        recent_command: None,
        permission_snapshot_id: PermissionSnapshotId(0),
        backend_badge: PaneSecurityBadge {
            level: PaneSecurityBadgeLevel::Normal,
            label: "cli".to_owned(),
            reasons: Vec::new(),
        },
        runtime_cell: None,
        response: None,
        apply_execution: None,
    })
}

fn cli_runtime_constraints(
    selected_skill_name: Option<&str>,
    home_root: &str,
    workspace_root: Option<&str>,
    workspace_instructions: &[WorkspaceInstruction],
    memory_hits: &[MemoryHybridHit],
    max_subagent_threads: usize,
    max_subagent_active_turns: usize,
) -> String {
    let mut constraints =
        agent_model_runtime_constraints(axon_agent_runtime::AgentModelRuntimeConstraintsInput {
            surface: "cli_run",
            selected_skill_name,
            home_root,
            workspace_root: workspace_root.unwrap_or(HOME_WITHOUT_WORKSPACE),
            workspace_instruction_paths: workspace_instructions
                .iter()
                .map(|instruction| instruction.path.clone())
                .collect(),
            capability_lines: Vec::new(),
            memory_hits,
        });
    constraints.push('\n');
    constraints.push_str(&axon_agent_runtime::agent_delegation_runtime_guidance(
        max_subagent_threads,
        0,
        max_subagent_active_turns,
        0,
    ));
    constraints
}

#[cfg(test)]
#[path = "run_model_payload_cache_replay_tests.rs"]
mod cache_replay_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use axon_agent_runtime::AgentModelToolCallMetadata;
    use axon_config::{AxonConfig, ConfigLoader};
    use axon_domain::{ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema};
    use std::{fs, path::Path};

    const TEST_MCP_TOOL_ID: ToolId = ToolId(9_001);

    #[test]
    fn model_history_preserves_order_and_cache_flags() {
        let history = serde_json::json!([
            {"role": "user", "content": "summary", "cache_excluded": true},
            {"role": "assistant", "content": "kept answer"}
        ])
        .to_string();

        let messages = model_history_messages(Some(&history)).unwrap();

        assert_eq!(
            messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["summary", "kept answer"]
        );
        assert!(messages[0].cache_excluded);
        assert!(!messages[1].cache_excluded);
    }

    #[test]
    fn runtime_constraints_do_not_present_home_as_workspace() {
        let home = "C:/Users/test/.axon";
        let constraints = cli_runtime_constraints(None, home, None, &[], &[], 16, 16);

        assert!(constraints.contains("个人 Home：C:/Users/test/.axon"));
        assert!(constraints.contains("当前工作地点（Workspace）：未选择（当前目录位于 Home）"));
        assert!(!constraints.contains("当前工作地点（Workspace）：C:/Users/test/.axon"));
    }

    #[test]
    fn environment_context_reports_axon_shell_not_windows_command_interpreter() {
        let context = cli_environment_context();
        let shell = context.shell.expect("环境感知必须提供 Axon 默认 Shell");
        let stem = Path::new(&shell)
            .file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);

        assert!(!matches!(stem.as_deref(), Some("cmd")));
        #[cfg(windows)]
        assert!(matches!(stem.as_deref(), Some("pwsh" | "powershell")));
    }

    #[test]
    fn structured_tool_history_uses_current_agent_message_schema() {
        let history = serde_json::to_string(&vec![
            AgentModelMessage::text("assistant", "").with_tool_calls(vec![
                AgentModelToolCallMetadata {
                    id: "call-1".to_owned(),
                    name: "mcp.search".to_owned(),
                    input: r#"{"query":"axon"}"#.into(),
                },
            ]),
            AgentModelMessage::tool_result("call-1", r#"{"hits":1}"#, true),
        ])
        .unwrap();

        let messages = model_history_messages(Some(&history)).unwrap();
        let call = messages[0]
            .provider_metadata
            .as_ref()
            .and_then(|metadata| metadata.tool_calls.as_ref())
            .and_then(|calls| calls.first())
            .expect("assistant 应保留结构化 tool call");
        let result = messages[1]
            .provider_metadata
            .as_ref()
            .and_then(|metadata| metadata.tool_result.as_ref())
            .expect("tool message 应保留结构化 result");

        assert_eq!(call.id, "call-1");
        assert_eq!(call.input.canonical_text(), r#"{"query":"axon"}"#);
        assert_eq!(result.call_id, "call-1");
        assert!(result.success);
    }

    #[test]
    fn cli_payload_injects_workspace_instructions_as_user_context() {
        let root = temp_root("prompt-fragments");
        let config_path = root.join("runtime").join("config.toml");
        let cwd = root.join("workspace");
        let skill_dir = cwd.join(".axon").join("skills").join("review");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("AGENTS.md"), "项目规则").unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: Review code\n---\nReview body",
        )
        .unwrap();
        ConfigLoader::save_to_path(&config_path, &AxonConfig::default()).unwrap();
        let mut startup = RunStartupSnapshot::build(&config_path, &cwd).unwrap();
        startup.personal_memory = Some(axon_memory_read::PersonalMemorySnapshot {
            id: "personal-memory-test".to_owned(),
            revision: 1,
            context: vec!["用户偏好中文".to_owned()],
        });
        startup.user_agents = Some(AgentModelMessage::context(
            "developer",
            "<user-agents-context>全局规则</user-agents-context>",
        ));

        let soul_message = AgentModelMessage::context(
            "system",
            "<assistant-profile-context>\n主代理 Soul\n</assistant-profile-context>",
        );
        let model_history = serde_json::to_string(&vec![AgentModelMessage::text(
            "assistant",
            "history-marker",
        )])
        .unwrap();
        let prompt = "please use skill://project/review on this diff";
        let skill_context = prepare_cli_run_skill_context(&startup, prompt, 128_000);
        let registry = startup.execution_tools_for_session(&config_path, "session-1");
        let registry = registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (payload, skill_selection) = build_cli_run_model_payload(
            CliRunModelPayloadInput {
                memory_hits: &[],
                soul_context_message: Some(&soul_message),
                persona_state_message: None,
                include_personal_memory: true,
                memory_disclosure_policy: DisclosurePolicy::PrivateOnly,
                cwd: &cwd.to_string_lossy(),
                workspace_snapshot: None,
                startup: &startup,
                stable_prefix: &startup.prompt_context.stable_prefix,
                context_window: 128_000,
                max_subagent_threads: 16,
                max_subagent_active_turns: 16,
                model_history_json: Some(&model_history),
                ledger_context: None,
            },
            skill_context,
            &registry,
        )
        .unwrap();

        assert_eq!(skill_selection.name.as_deref(), Some("review"));
        let user_message = payload
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .expect("payload 应包含最终 user message");
        assert_eq!(user_message.content, "please use  on this diff");
        assert!(!user_message.content.contains("Review body"));
        let workspace_message = payload
            .messages
            .iter()
            .find(|message| {
                message.role == "developer"
                    && message.content.contains("# AGENTS.md instructions")
                    && message.content.contains("<INSTRUCTIONS>")
            })
            .expect("payload 应包含 AGENTS.md developer context");
        assert!(workspace_message.content.contains("项目规则"));
        assert!(workspace_message.cache_excluded);
        let workspace_index = message_index(&payload, "# AGENTS.md instructions");
        let personal_index = message_index(&payload, "<personal-memory-context>");
        let user_agents_index = message_index(&payload, "<user-agents-context>");
        let skills_index = message_index(&payload, "<available-skills-context>");
        let selected_skill_index = message_index(&payload, "<selected-skill-context>");
        let final_user_index = payload.messages.len() - 1;
        assert!(workspace_index < skills_index);
        assert!(skills_index < selected_skill_index);
        assert!(selected_skill_index < final_user_index);
        assert!(payload.messages[skills_index].cache_excluded);
        let soul_index = message_index(&payload, "<assistant-profile-context>");
        let pane_index = message_index(&payload, "<agent-pane-context>");
        let context_snapshot_index = message_index(&payload, "<agent-context-snapshot>");
        let history_index = payload
            .messages
            .iter()
            .position(|message| message.content == "history-marker")
            .expect("payload 应包含模型历史");
        assert_eq!(payload.messages[soul_index].role, "system");
        assert!(!payload.messages[soul_index].cache_excluded);
        assert!(soul_index < user_agents_index);
        assert!(user_agents_index < personal_index);
        assert!(personal_index < history_index);
        assert!(history_index < workspace_index);
        assert!(workspace_index < skills_index);
        assert!(skills_index < context_snapshot_index);
        assert!(context_snapshot_index < pane_index);
        assert!(payload.messages[context_snapshot_index].cache_excluded);
        assert!(payload.messages[pane_index].cache_excluded);
        assert!(history_index < selected_skill_index);
        assert!(selected_skill_index < final_user_index);
        assert!(payload.messages[selected_skill_index].cache_excluded);
        let routing_message = payload
            .messages
            .iter()
            .find(|message| message.content.contains("<capability-routing>"))
            .expect("payload 应包含当前轮次能力路由");
        assert_eq!(routing_message.role, "developer");
        assert!(routing_message.cache_excluded);
        let routing_index = payload
            .messages
            .iter()
            .position(|message| message.content.contains("<capability-routing>"))
            .unwrap();
        assert!(history_index < routing_index);
        assert!(routing_index < final_user_index);
        let environment_index = message_index(&payload, "<environment-context>");
        assert!(history_index < environment_index);
        assert!(environment_index < context_snapshot_index);
        assert!(payload.messages[environment_index].cache_excluded);
        assert_eq!(
            payload.messages[environment_index]
                .content
                .matches("workspace")
                .count(),
            0,
            "环境补充不得重复注入 Workspace"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cli_payload_exposes_mcp_tools_without_deferred_catalog() {
        let root = temp_root("deferred-mcp-routing");
        let config_path = root.join("runtime").join("config.toml");
        let cwd = root.join("workspace");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        ConfigLoader::save_to_path(&config_path, &AxonConfig::default()).unwrap();
        let mut startup = RunStartupSnapshot::build(&config_path, &cwd).unwrap();
        startup.personal_memory = Some(axon_memory_read::PersonalMemorySnapshot {
            id: "personal-memory-subagent-test".to_owned(),
            revision: 1,
            context: vec!["不应注入子代理".to_owned()],
        });
        startup.execution_tools.register(test_mcp_tool()).unwrap();

        let prompt = "query the project database";
        let skill_context = prepare_cli_run_skill_context(&startup, prompt, 128_000);
        let registry = startup.execution_tools_for_session(&config_path, "session-mcp");
        let registry = registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (payload, _) = build_cli_run_model_payload(
            CliRunModelPayloadInput {
                memory_hits: &[],
                soul_context_message: None,
                persona_state_message: None,
                include_personal_memory: false,
                memory_disclosure_policy: DisclosurePolicy::Workspace,
                cwd: &cwd.to_string_lossy(),
                workspace_snapshot: None,
                startup: &startup,
                stable_prefix: &startup.prompt_context.stable_prefix,
                context_window: 128_000,
                max_subagent_threads: 16,
                max_subagent_active_turns: 16,
                model_history_json: None,
                ledger_context: None,
            },
            skill_context,
            &registry,
        )
        .unwrap();

        assert!(
            payload
                .messages
                .iter()
                .all(|message| !message.content.contains("<personal-memory-context>"))
        );
        assert!(
            payload
                .messages
                .iter()
                .all(|message| !message.content.contains("[Deferred MCP")),
            "MCP schema 已全量下发，payload 不得再出现 deferred 目录或候选"
        );
        assert!(
            payload
                .model_tools
                .iter()
                .any(|tool| tool.qualified_name == "mcp__team__query_db"),
            "会话注册表内的 MCP 工具必须直接进入 provider 工具列表"
        );
        let _ = fs::remove_dir_all(root);
    }

    fn test_mcp_tool() -> ToolDefinition {
        ToolDefinition {
            id: TEST_MCP_TOOL_ID,
            namespace: "mcp__team".to_owned(),
            name: "query_db".to_owned(),
            description: "Query the project database".to_owned(),
            schema: ToolSchema::new(
                1,
                serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
            ),
            enabled: true,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            output_policy: ToolOutputPolicy::BoundedSummary,
            permission_policy: ToolPermissionPolicy::ReadOnly,
        }
    }

    fn message_index(payload: &AgentModelPayload, marker: &str) -> usize {
        payload
            .messages
            .iter()
            .position(|message| {
                matches!(message.role.as_str(), "system" | "developer")
                    && message.content.contains(marker)
            })
            .unwrap_or_else(|| panic!("payload 应包含 stable context marker {marker}"))
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        axon_test_support::test_temp_root()
            .join(format!("axon-cli-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn cli_payload_carries_persona_state_only_when_provided() {
        let root = temp_root("persona-state");
        let config_path = root.join("runtime").join("config.toml");
        let cwd = root.join("workspace");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        ConfigLoader::save_to_path(&config_path, &AxonConfig::default()).unwrap();
        let startup = RunStartupSnapshot::build(&config_path, &cwd).unwrap();
        let prompt = "继续";
        let registry = startup.execution_tools_for_session(&config_path, "session-1");
        let registry = registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let persona_message = AgentModelMessage::cache_excluded(
            "developer",
            "<persona-state-context>\nmode: casual\n</persona-state-context>",
        );

        let build = |persona: Option<&AgentModelMessage>| {
            let (payload, _) = build_cli_run_model_payload(
                CliRunModelPayloadInput {
                    memory_hits: &[],
                    soul_context_message: None,
                    persona_state_message: persona,
                    include_personal_memory: false,
                    memory_disclosure_policy: DisclosurePolicy::PrivateOnly,
                    cwd: &cwd.to_string_lossy(),
                    workspace_snapshot: None,
                    startup: &startup,
                    stable_prefix: &startup.prompt_context.stable_prefix,
                    context_window: 128_000,
                    max_subagent_threads: 16,
                    max_subagent_active_turns: 16,
                    model_history_json: None,
                    ledger_context: None,
                },
                prepare_cli_run_skill_context(&startup, prompt, 128_000),
                &registry,
            )
            .unwrap();
            payload
        };

        let with_persona = build(Some(&persona_message));
        let without_persona = build(None);

        assert!(
            with_persona
                .messages
                .iter()
                .any(|message| message.content.contains("<persona-state-context>")),
            "提供人格状态段时 payload 必须带上它"
        );
        assert!(
            !without_persona
                .messages
                .iter()
                .any(|message| message.content.contains("<persona-state-context>")),
            "未提供时不得凭空出现人格状态段"
        );
    }
}
