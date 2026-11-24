use super::{
    setup_interactive::InteractiveSetupInput,
    setup_interactive_config::AgentSetupConfig,
    setup_plan::{ExplicitEntry, build_setup_plan},
    setup_plan_entries::collect_explicit_entries,
};
use crate::{AxonCli, SetupCommand};
use axon_config::{
    AxonConfig, DEFAULT_MODEL_CONTEXT_WINDOW, DEFAULT_MODEL_MAX_OUTPUT_TOKENS, ModelConfig,
    ProviderConfig,
};

fn empty_setup_command() -> SetupCommand {
    let cli = AxonCli::parse_from_args(["axon", "setup", "--non-interactive"]);
    let Some(crate::AxonCommand::Setup(command)) = cli.command else {
        panic!("未解析到 setup 命令");
    };
    command
}

#[test]
fn interactive_clear_flags_create_clear_entries() {
    let input = InteractiveSetupInput {
        clear_websocket_url: true,
        clear_api_key: true,
        clear_reasoning_effort: true,
        clear_max_output_tokens: true,
        ..InteractiveSetupInput::default()
    };
    let entries =
        collect_explicit_entries(&empty_setup_command(), &AgentSetupConfig::default(), &input)
            .unwrap();

    assert!(matches!(entries.websocket_url, ExplicitEntry::Clear));
    assert!(matches!(entries.api_key, ExplicitEntry::Clear));
    assert!(matches!(
        entries.agent_reasoning_effort,
        ExplicitEntry::Clear
    ));
    assert!(matches!(entries.max_output_tokens, ExplicitEntry::Clear));
}

#[test]
fn untouched_interactive_defaults_leave_the_plan_unchanged() {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "existing",
        "openai",
        "https://example.test/v1",
    ));
    config.models.push(ModelConfig {
        name: "existing-model".to_owned(),
        id: "model-id".to_owned(),
        provider: "existing".to_owned(),
        context_window: DEFAULT_MODEL_CONTEXT_WINDOW,
        max_output_tokens: Some(DEFAULT_MODEL_MAX_OUTPUT_TOKENS),
        supports_vision: true,
        supports_embedding: false,
        embedding_dimension: 0,
        requires_reasoning_content_on_assistant_messages: false,
    });

    let plan = build_setup_plan(
        &empty_setup_command(),
        &AgentSetupConfig::default(),
        &InteractiveSetupInput {
            interactive: true,
            ..InteractiveSetupInput::default()
        },
        &config,
    )
    .unwrap();

    assert_eq!(plan.candidate_config, config);
    assert!(plan.env_changes.is_empty());
    assert!(plan.changed_summary.is_empty());
}

#[test]
fn selected_provider_without_edits_only_changes_the_interactive_target() {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "first",
        "openai",
        "https://first.example.test/v1",
    ));
    config.providers.push(ProviderConfig::new(
        "second",
        "openai",
        "https://second.example.test/v1",
    ));

    let plan = build_setup_plan(
        &empty_setup_command(),
        &AgentSetupConfig::default(),
        &InteractiveSetupInput {
            interactive: true,
            selected_provider_name: Some("second".to_owned()),
            ..InteractiveSetupInput::default()
        },
        &config,
    )
    .unwrap();

    assert_eq!(plan.candidate_config, config);
    assert!(plan.changed_summary.is_empty());
    assert_eq!(plan.effective_provider_name, "second");
}

#[test]
fn interactive_provider_routing_fields_update_the_selected_provider() {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "provider",
        "openai",
        "https://example.test/v1",
    ));

    let plan = build_setup_plan(
        &empty_setup_command(),
        &AgentSetupConfig::default(),
        &InteractiveSetupInput {
            interactive: true,
            selected_provider_name: Some("provider".to_owned()),
            provider_enabled: Some(false),
            provider_priority: Some(42),
            provider_weight: Some(7),
            ..InteractiveSetupInput::default()
        },
        &config,
    )
    .unwrap();

    let provider = plan
        .candidate_config
        .providers
        .iter()
        .find(|provider| provider.name == "provider")
        .unwrap();
    assert!(!provider.enabled);
    assert_eq!(provider.priority, 42);
    assert_eq!(provider.weight, 7);
}

#[test]
fn selected_model_updates_agent_slots_only_after_plan_commit() {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "provider",
        "openai",
        "https://example.test/v1",
    ));
    for (name, id) in [("first", "first-id"), ("second", "second-id")] {
        config.models.push(ModelConfig {
            name: name.to_owned(),
            id: id.to_owned(),
            provider: "provider".to_owned(),
            context_window: DEFAULT_MODEL_CONTEXT_WINDOW,
            max_output_tokens: Some(DEFAULT_MODEL_MAX_OUTPUT_TOKENS),
            supports_vision: true,
            supports_embedding: false,
            embedding_dimension: 0,
            requires_reasoning_content_on_assistant_messages: false,
        });
    }

    let plan = build_setup_plan(
        &empty_setup_command(),
        &AgentSetupConfig::default(),
        &InteractiveSetupInput {
            interactive: true,
            selected_provider_name: Some("provider".to_owned()),
            model: Some("second-id".to_owned()),
            model_name: Some("second".to_owned()),
            configure_model: Some(true),
            ..InteractiveSetupInput::default()
        },
        &config,
    )
    .unwrap();

    assert_eq!(plan.candidate_config.agent.default_model, "second");
    assert_eq!(plan.candidate_config.agent.fast_model, "second");
}
