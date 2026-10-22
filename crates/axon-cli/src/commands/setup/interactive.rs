use std::{
    collections::BTreeMap,
    io::{self, IsTerminal},
    path::Path,
};

use crate::{OutputFormat, SetupCommand};
use axon_config::{AxonConfig, ModelConfig};

use super::setup_interactive_canvas::{
    ESCAPE_CHOICE, read_frame_choice, read_menu_choice, render_setup_frame,
};
use super::setup_interactive_config::collect_module_config_values;
use super::setup_interactive_model_catalog::collect_model_input;
use super::setup_interactive_provider_catalog::collect_provider_input;
use super::setup_interactive_style::{SetupStyle, paint};
use super::setup_interactive_views::{
    collect_advanced_config_input, collect_remote_input, collect_reset_menu_input, print_dashboard,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InteractiveSetupInput {
    pub interactive: bool,
    pub cancelled: bool,
    pub selected_provider_name: Option<String>,
    pub provider_name: Option<String>,
    pub protocol: Option<String>,
    pub base_url: Option<String>,
    pub wire_transport: Option<String>,
    pub websocket_url: Option<String>,
    pub provider_enabled: Option<bool>,
    pub provider_priority: Option<i32>,
    pub provider_weight: Option<u32>,
    pub selected_model_id: Option<String>,
    pub selected_model_provider: Option<String>,
    pub model: Option<String>,
    pub model_name: Option<String>,
    pub model_provider: Option<String>,
    pub agent_reasoning_effort: Option<String>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_vision: Option<bool>,
    pub supports_embedding: Option<bool>,
    pub embedding_dimension: Option<usize>,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
    pub clear_websocket_url: bool,
    pub clear_api_key: bool,
    pub clear_reasoning_effort: bool,
    pub clear_max_output_tokens: bool,
    pub configure_provider: Option<bool>,
    pub configure_model: Option<bool>,
    pub reset_provider: Option<String>,
    pub reset_agent_models: bool,
    pub config_values: BTreeMap<String, String>,
}

#[derive(Default)]
pub(super) struct ModelInteractiveInput {
    pub(super) selected_model_id: Option<String>,
    pub(super) selected_model_provider: Option<String>,
    pub(super) model: Option<String>,
    pub(super) model_name: Option<String>,
    pub(super) model_provider: Option<String>,
    pub(super) agent_reasoning_effort: Option<String>,
    pub(super) context_window: Option<u64>,
    pub(super) max_output_tokens: Option<u64>,
    pub(super) supports_vision: Option<bool>,
    pub(super) supports_embedding: Option<bool>,
    pub(super) embedding_dimension: Option<usize>,
    pub(super) clear_reasoning_effort: bool,
    pub(super) clear_max_output_tokens: bool,
    pub(super) configure_model: bool,
}

#[derive(Default)]
pub(super) struct ProviderInteractiveInput {
    pub(super) provider_name: Option<String>,
    pub(super) resolved_provider_name: String,
    pub(super) protocol: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) wire_transport: Option<String>,
    pub(super) websocket_url: Option<String>,
    pub(super) api_key_env: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) clear_websocket_url: bool,
    pub(super) clear_api_key: bool,
    pub(super) configure_provider: bool,
    pub(super) provider_enabled: Option<bool>,
    pub(super) provider_priority: Option<i32>,
    pub(super) provider_weight: Option<u32>,
}

#[derive(Default)]
pub(super) struct SetupResetInteractiveInput {
    pub(super) reset_provider: Option<String>,
    pub(super) reset_agent_models: bool,
}

pub(crate) fn collect_interactive_setup_input(
    command: &SetupCommand,
    format: OutputFormat,
    config: &AxonConfig,
    env_path: &Path,
) -> InteractiveSetupInput {
    if command.non_interactive || format == OutputFormat::Json || !io::stdin().is_terminal() {
        return InteractiveSetupInput::default();
    }

    let initial_provider_name = default_provider_name(config, command);
    let mut provider = ProviderInteractiveInput {
        resolved_provider_name: initial_provider_name.clone(),
        ..ProviderInteractiveInput::default()
    };
    let mut model = ModelInteractiveInput::default();
    let mut reset_actions = SetupResetInteractiveInput::default();
    let mut config_values = BTreeMap::new();
    let mut cancelled = false;

    loop {
        let choices = ["1", "2", "3", "4", "5", "R", "S", "Q"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let choice = read_menu_choice(
            |prompt, focus, selected| print_dashboard(config, env_path, prompt, focus, selected),
            "选择操作",
            &choices,
            6,
            false,
        );
        match choice.to_ascii_uppercase().as_str() {
            "1" | "1." | "PROVIDERS" => {
                collect_provider_input(command, config, env_path, &mut provider);
            }
            "2" | "2." | "MODELS" => {
                collect_model_input(
                    command,
                    config,
                    &provider.resolved_provider_name,
                    &mut model,
                );
            }
            "3" | "3." | "AGENT" => {
                collect_module_config_values(config, "agent", &mut config_values);
            }
            "4" | "4." | "REMOTE" => {
                collect_remote_input(config, &mut config_values);
            }
            "5" | "5." | "ADVANCED" => {
                collect_advanced_config_input(config, &mut config_values);
            }
            "R" | "RESET" => {
                collect_reset_menu_input(&mut reset_actions);
            }
            "S" | "SAVE" | "S." => break,
            "Q" | "CANCEL" | "Q." => {
                cancelled = true;
                break;
            }
            ESCAPE_CHOICE
                if should_discard_pending_changes(
                    &provider,
                    &model,
                    &reset_actions,
                    &config_values,
                ) =>
            {
                cancelled = true;
                break;
            }
            _ => {}
        }
    }

    InteractiveSetupInput {
        interactive: true,
        cancelled,
        selected_provider_name: Some(provider.resolved_provider_name),
        provider_name: provider.provider_name,
        protocol: provider.protocol,
        base_url: provider.base_url,
        wire_transport: provider.wire_transport,
        websocket_url: provider.websocket_url,
        provider_enabled: provider.provider_enabled,
        provider_priority: provider.provider_priority,
        provider_weight: provider.provider_weight,
        selected_model_id: model.selected_model_id,
        selected_model_provider: model.selected_model_provider,
        model: model.model,
        model_name: model.model_name,
        model_provider: model.model_provider,
        agent_reasoning_effort: model.agent_reasoning_effort,
        context_window: model.context_window,
        max_output_tokens: model.max_output_tokens,
        supports_vision: model.supports_vision,
        supports_embedding: model.supports_embedding,
        embedding_dimension: model.embedding_dimension,
        api_key_env: provider.api_key_env,
        api_key: provider.api_key,
        clear_websocket_url: provider.clear_websocket_url,
        clear_api_key: provider.clear_api_key,
        clear_reasoning_effort: model.clear_reasoning_effort,
        clear_max_output_tokens: model.clear_max_output_tokens,
        configure_provider: provider.configure_provider.then_some(true),
        configure_model: model.configure_model.then_some(true),
        reset_provider: reset_actions.reset_provider,
        reset_agent_models: reset_actions.reset_agent_models,
        config_values,
    }
}

fn should_discard_pending_changes(
    provider: &ProviderInteractiveInput,
    model: &ModelInteractiveInput,
    reset_actions: &SetupResetInteractiveInput,
    config_values: &BTreeMap<String, String>,
) -> bool {
    if !has_pending_changes(provider, model, reset_actions, config_values) {
        return true;
    }
    read_frame_choice(
        |input| {
            let lines = vec![
                paint(SetupStyle::Error, "存在未保存的配置变更。"),
                String::new(),
                paint(SetupStyle::Muted, "再次按 Esc 放弃变更并退出。"),
                paint(SetupStyle::Muted, "按 Enter 返回主菜单继续编辑。"),
                String::new(),
                paint(SetupStyle::Accent, input),
            ];
            render_setup_frame(&paint(SetupStyle::Accent, "DISCARD CHANGES"), &lines);
        },
        "确认退出",
    ) == ESCAPE_CHOICE
}

fn has_pending_changes(
    provider: &ProviderInteractiveInput,
    model: &ModelInteractiveInput,
    reset_actions: &SetupResetInteractiveInput,
    config_values: &BTreeMap<String, String>,
) -> bool {
    provider.configure_provider
        || model.configure_model
        || provider.clear_websocket_url
        || provider.clear_api_key
        || reset_actions.reset_provider.is_some()
        || reset_actions.reset_agent_models
        || !config_values.is_empty()
}

pub(crate) fn provider_protocol_summary(config: &AxonConfig, provider_name: &str) -> String {
    config
        .providers
        .iter()
        .find(|provider| provider.name == provider_name)
        .map_or_else(
            || "未配置".to_owned(),
            |provider| format!("{} via {}", provider.protocol, provider.wire_transport),
        )
}

pub(crate) fn model_summary(config: &AxonConfig, provider_name: &str, model_id: &str) -> String {
    config
        .models
        .iter()
        .find(|model| model_matches(model, provider_name, model_id))
        .map_or_else(
            || "未配置".to_owned(),
            |model| {
                format!(
                    "ctx={} vision={} embedding={}",
                    model.context_window, model.supports_vision, model.supports_embedding
                )
            },
        )
}

fn model_matches(model: &ModelConfig, provider_name: &str, model_id: &str) -> bool {
    model.provider == provider_name && (model.id == model_id || model.name == model_id)
}

fn default_provider_name(config: &AxonConfig, command: &SetupCommand) -> String {
    config.providers.first().map_or_else(
        || {
            command
                .provider_name
                .clone()
                .unwrap_or_else(|| "openai".to_owned())
        },
        |provider| provider.name.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_provider_configuration_requires_discard_confirmation() {
        let provider = ProviderInteractiveInput {
            configure_provider: true,
            ..ProviderInteractiveInput::default()
        };

        assert!(has_pending_changes(
            &provider,
            &ModelInteractiveInput::default(),
            &SetupResetInteractiveInput::default(),
            &BTreeMap::new(),
        ));
    }

    #[test]
    fn untouched_setup_does_not_require_discard_confirmation() {
        assert!(!has_pending_changes(
            &ProviderInteractiveInput::default(),
            &ModelInteractiveInput::default(),
            &SetupResetInteractiveInput::default(),
            &BTreeMap::new(),
        ));
    }
}
