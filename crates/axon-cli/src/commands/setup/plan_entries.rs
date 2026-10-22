use super::{
    setup_interactive::InteractiveSetupInput,
    setup_interactive_config::AgentSetupConfig,
    setup_plan::{ExplicitEntry, SetupExplicitEntries},
};
use crate::SetupCommand;
use std::collections::BTreeMap;

pub(crate) fn collect_explicit_entries(
    cmd: &SetupCommand,
    agent_cfg: &AgentSetupConfig,
    interactive_input: &InteractiveSetupInput,
) -> Result<SetupExplicitEntries, String> {
    for key in agent_cfg.config_values.keys() {
        if matches!(
            key.as_str(),
            "agent.default_model" | "agent.fast_model" | "agent.reasoning_effort"
        ) {
            return Err(format!(
                "configValues 包含已被结构化 Setup 字段表示的 key: '{key}'"
            ));
        }
    }

    let mut config_values = BTreeMap::new();
    for (key, value) in &interactive_input.config_values {
        config_values.insert(key.clone(), ExplicitEntry::Set(value.clone()));
    }
    for (key, value) in &agent_cfg.config_values {
        config_values.insert(key.clone(), ExplicitEntry::Set(value.clone()));
    }

    Ok(SetupExplicitEntries {
        provider_name: merge_string_entry(
            cmd.provider_name.as_deref(),
            false,
            agent_cfg.provider_name.as_deref(),
            false,
            interactive_input.provider_name.as_deref(),
        ),
        protocol: merge_string_entry(
            cmd.protocol.map(|value| value.as_config_value()),
            false,
            agent_cfg.protocol.as_deref(),
            false,
            interactive_input.protocol.as_deref(),
        ),
        base_url: merge_string_entry(
            cmd.base_url.as_deref(),
            false,
            agent_cfg.base_url.as_deref(),
            false,
            interactive_input.base_url.as_deref(),
        ),
        wire_transport: merge_string_entry(
            cmd.wire_transport.map(|value| value.as_config_value()),
            false,
            agent_cfg.wire_transport.as_deref(),
            false,
            interactive_input.wire_transport.as_deref(),
        ),
        websocket_url: merge_string_entry(
            cmd.websocket_url.as_deref(),
            cmd.clear_websocket_url || interactive_input.clear_websocket_url,
            agent_cfg.websocket_url.as_deref(),
            agent_cfg.is_clear_field("websocketUrl"),
            interactive_input.websocket_url.as_deref(),
        ),
        provider_enabled: merge_pod_entry(
            None,
            false,
            agent_cfg.provider_enabled,
            false,
            interactive_input.provider_enabled,
        ),
        provider_priority: merge_pod_entry(
            None,
            false,
            agent_cfg.provider_priority,
            false,
            interactive_input.provider_priority,
        ),
        provider_weight: merge_pod_entry(
            None,
            false,
            agent_cfg.provider_weight,
            false,
            interactive_input.provider_weight,
        ),
        model_id: merge_string_entry(
            cmd.model.as_deref(),
            false,
            agent_cfg.model.as_deref(),
            false,
            interactive_input.model.as_deref(),
        ),
        model_name: merge_string_entry(
            cmd.model_name.as_deref(),
            false,
            agent_cfg.model_name.as_deref(),
            false,
            interactive_input.model_name.as_deref(),
        ),
        model_provider: merge_string_entry(
            None,
            false,
            None,
            false,
            interactive_input.model_provider.as_deref(),
        ),
        agent_reasoning_effort: merge_string_entry(
            cmd.reasoning_effort.as_deref(),
            cmd.clear_reasoning_effort || interactive_input.clear_reasoning_effort,
            agent_cfg.agent_reasoning_effort.as_deref(),
            agent_cfg.is_clear_field("reasoningEffort"),
            interactive_input.agent_reasoning_effort.as_deref(),
        ),
        context_window: merge_pod_entry(
            cmd.context_window,
            false,
            agent_cfg.context_window,
            false,
            interactive_input.context_window,
        ),
        max_output_tokens: merge_pod_entry(
            cmd.max_output_tokens,
            cmd.clear_max_output_tokens || interactive_input.clear_max_output_tokens,
            agent_cfg.max_output_tokens,
            agent_cfg.is_clear_field("maxOutputTokens"),
            interactive_input.max_output_tokens,
        ),
        supports_vision: merge_pod_entry(
            cmd.supports_vision,
            false,
            agent_cfg.supports_vision,
            false,
            interactive_input.supports_vision,
        ),
        supports_embedding: merge_pod_entry(
            cmd.supports_embedding,
            false,
            agent_cfg.supports_embedding,
            false,
            interactive_input.supports_embedding,
        ),
        embedding_dimension: merge_pod_entry(
            cmd.embedding_dimension,
            false,
            agent_cfg.embedding_dimension,
            false,
            interactive_input.embedding_dimension,
        ),
        api_key_env: merge_string_entry(
            cmd.api_key_env.as_deref(),
            false,
            agent_cfg.api_key_env.as_deref(),
            false,
            interactive_input.api_key_env.as_deref(),
        ),
        api_key: merge_string_entry(
            cmd.api_key.as_deref(),
            cmd.clear_api_key || interactive_input.clear_api_key,
            agent_cfg.api_key.as_deref(),
            agent_cfg.is_clear_field("apiKey"),
            interactive_input.api_key.as_deref(),
        ),
        config_values,
    })
}

fn merge_string_entry(
    cmd_val: Option<&str>,
    cmd_clear: bool,
    agent_val: Option<&str>,
    agent_clear: bool,
    interactive_val: Option<&str>,
) -> ExplicitEntry<String> {
    if cmd_clear {
        ExplicitEntry::Clear
    } else if let Some(value) = cmd_val {
        ExplicitEntry::Set(value.to_owned())
    } else if agent_clear {
        ExplicitEntry::Clear
    } else if let Some(value) = agent_val {
        ExplicitEntry::Set(value.to_owned())
    } else if let Some(value) = interactive_val {
        ExplicitEntry::Set(value.to_owned())
    } else {
        ExplicitEntry::Unchanged
    }
}

fn merge_pod_entry<T: Copy>(
    cmd_val: Option<T>,
    cmd_clear: bool,
    agent_val: Option<T>,
    agent_clear: bool,
    interactive_val: Option<T>,
) -> ExplicitEntry<T> {
    if cmd_clear {
        ExplicitEntry::Clear
    } else if let Some(value) = cmd_val {
        ExplicitEntry::Set(value)
    } else if agent_clear {
        ExplicitEntry::Clear
    } else if let Some(value) = agent_val {
        ExplicitEntry::Set(value)
    } else if let Some(value) = interactive_val {
        ExplicitEntry::Set(value)
    } else {
        ExplicitEntry::Unchanged
    }
}
