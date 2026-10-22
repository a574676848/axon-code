//! 普通 Coding `RunRequest` 的统一构建流程。

use super::*;

impl RunRequest {
    pub(in crate::run_commands) fn from_command(
        command: &RunCommand,
        config: &AxonConfig,
        config_path: &Path,
        session: RunRequestSession<'_>,
        cwd: Option<&Path>,
        options: RunRequestBuildOptions,
    ) -> Result<Self, String> {
        Self::build(command, config, config_path, session, cwd, None, options)
    }

    fn build(
        command: &RunCommand,
        config: &AxonConfig,
        config_path: &Path,
        session: RunRequestSession<'_>,
        cwd: Option<&Path>,
        resolved_channel: Option<(&str, &str)>,
        options: RunRequestBuildOptions,
    ) -> Result<Self, String> {
        let startup = options.startup;
        let cancel_token = options.cancel_token;
        let subagent_event_bus = options.subagent_event_bus;
        let subagent_session_sinks = options.subagent_session_sinks;
        let agent_identity = options.agent_identity;
        let client_channel = options.client_channel;
        let performance = options.performance;
        let (prepared, previous_session) = session.prepare(command, config, cwd)?;
        let command = &prepared.command;
        let cwd = prepared.cwd;
        let prompt = prepared.prompt;
        let attached_files = prepared.attached_files;
        let session_id = prepared.session_id;
        let model_history_json = prepared.model_history_json;
        let balanced = match resolved_channel {
            Some((provider_name, model_id)) => {
                resolved_request_channel(config, provider_name, model_id)?
            }
            None => select_request_channel(command, config, &session_id, agent_identity)?,
        };
        let channel_chain = balanced.chain;
        let model = balanced.model;
        // 快速子代理不继承主 turn 的思考强度：交由模型自身默认值决定。
        let reasoning_effort = if agent_identity.is_primary() {
            Some(resolve_run_reasoning_effort(command, config))
        } else {
            None
        };
        let provider = balanced.provider;
        let routing_key = balanced.routing_key;
        let model_profile_id = resolve_model_profile_id(config, &provider.name, &model.name)
            .unwrap_or(CLI_RUN_DEFAULT_MODEL_PROFILE_ID);
        validate_run_provider(&provider)?;
        // 快速子代理不装配主 turn 工具策略：使用按身份裁剪的稳定前缀。
        let stable_prefix = if agent_identity.is_primary() {
            &startup.prompt_context.stable_prefix
        } else {
            &startup.prompt_context.subagent_stable_prefix
        };
        let runtime_input = BuildRunModelPayloadInput {
            config_path,
            config,
            cwd: &cwd,
            command,
            startup: &startup,
            stable_prefix,
            prompt_for_model: &prompt,
            session_id: &session_id,
            model_history_json: model_history_json.as_deref(),
            attached_files: &attached_files,
            supports_vision: model.supports_vision,
            context_window: model.context_window,
            include_user_soul: agent_identity.includes_user_soul(),
            include_personal_memory: agent_identity.includes_personal_memory(),
            memory_disclosure_policy: agent_identity.memory_disclosure_policy(),
            performance: performance.as_deref(),
            workspace_snapshot_prewarm: options.workspace_snapshot_prewarm,
        };
        let (model_input, execution_tools) = prepare_model_input_and_tools(&runtime_input)?;
        let workspace_files = Arc::new(WorkspaceFileSession::default());
        let subagents = run_subagent_execution_wiring(RunSubagentExecutionWiringInput {
            config,
            config_path,
            session_id: &session_id,
            cwd: &cwd,
            provider: &provider,
            model: &model,
            reasoning_effort: reasoning_effort.clone(),
            channel_chain: &channel_chain,
            routing_key: &routing_key,
            command,
            startup: &startup,
            cancel_token: cancel_token.clone(),
            workspace_files: Arc::clone(&workspace_files),
            subagent_event_bus,
            subagent_session_sinks,
        })?;
        Ok(assemble_run_request(RunRequestAssemblyInput {
            client_channel,
            config_path: config_path.to_path_buf(),
            session_id,
            resumed_from: previous_session.map(|session| session.record.session_id.clone()),
            parent_session_id: prepared.parent_session_id,
            cwd,
            prompt,
            structured_output: None,
            model_input,
            startup,
            execution_tools,
            workspace_files,
            provider,
            model,
            reasoning_effort,
            channel_chain,
            routing_key,
            balancer: config.channel_balancer,
            timeout: provider_stream_idle_timeout(command.timeout),
            cancel_token,
            model_profile_id,
            permission_snapshot_id: CLI_RUN_DEFAULT_PERMISSION_SNAPSHOT_ID,
            current_agent_path: AgentPath::root(),
            agent_control: subagents.agent_control,
            agent_graph_store: subagents.agent_graph_store,
            subagent_sessions: subagents.subagent_sessions,
            subagent_executor: subagents.subagent_executor,
            subagent_cancel_tokens: subagents.subagent_cancel_tokens,
            steer_inbox: None,
            auto_compaction_threshold: config.agent.auto_compaction_threshold,
            performance,
        }))
    }
}

pub(in crate::run_commands) fn resolved_request_channel(
    config: &AxonConfig,
    provider_name: &str,
    model_id: &str,
) -> Result<ChannelBalancedModel, String> {
    let model = config
        .models
        .iter()
        .find(|model| {
            config.model_is_enabled(model)
                && model.provider == provider_name
                && model.id == model_id
        })
        .cloned()
        .ok_or_else(|| "恢复的普通 Coding Provider/模型已不在当前启用配置中".to_owned())?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.enabled && provider.name == provider_name)
        .cloned()
        .ok_or_else(|| "恢复的普通 Coding Provider 已被禁用或删除".to_owned())?;
    Ok(ChannelBalancedModel {
        routing_key: axon_agent_runtime::model_routing_key(&model).to_owned(),
        chain: vec![(model.clone(), provider.clone())],
        model,
        provider,
    })
}

fn select_request_channel(
    command: &RunCommand,
    config: &AxonConfig,
    session_id: &str,
    agent_identity: RunAgentIdentity,
) -> Result<ChannelBalancedModel, String> {
    let session_selector = hash_session_selector(session_id);
    if agent_identity.is_primary() {
        select_channel_balanced_model(command, config, session_id, session_selector, now_ms())
    } else {
        select_channel_balanced_model_for_agent_slot(
            config,
            &config.agent.fast_model,
            FAST_MODEL_SLOT_NAME,
            session_id,
            session_selector,
            now_ms(),
        )
    }
}
