use super::{
    AxonConfig, ConfigLoader, ModelConfig, ProviderCapabilities, ProviderConfig, add_provider,
    remove_provider, to_toml_string,
};

#[test]
fn provider_enabled_defaults_to_true_and_disabled_state_round_trips() {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-provider-enabled-default-{}.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        r#"
[agent]
default_model = "chat"
fast_model = "chat"

[[providers]]
name = "enabled-provider"
protocol = "openai"
base_url = "https://enabled.invalid/v1"

[[providers]]
name = "disabled-provider"
enabled = false
protocol = "openai"
base_url = "https://disabled.invalid/v1"

[[models]]
name = "chat"
id = "chat-enabled"
provider = "enabled-provider"
context_window = 32000

[[models]]
name = "chat-disabled"
id = "chat-disabled"
provider = "disabled-provider"
context_window = 32000
"#,
    )
    .unwrap();

    let config = ConfigLoader::load_from_path(&path).unwrap().config;
    let serialized = to_toml_string(&config).unwrap();
    let enabled_model = &config.models[0];
    let disabled_model = &config.models[1];

    assert!(config.providers[0].enabled);
    assert!(!config.providers[1].enabled);
    assert!(config.model_is_enabled(enabled_model));
    assert!(!config.model_is_enabled(disabled_model));
    assert!(serialized.contains("enabled = false"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn provider_and_model_catalog_validates_references() {
    let mut config = AxonConfig::default();
    add_provider(
        &mut config,
        ProviderConfig::new("local", "openai", "http://127.0.0.1:11434/v1"),
    )
    .unwrap();
    config.models.push(ModelConfig {
        name: "local-chat".to_owned(),
        id: "local-chat".to_owned(),
        provider: "local".to_owned(),
        context_window: 32_000,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: false,
        embedding_dimension: 0,
        requires_reasoning_content_on_assistant_messages: false,
    });
    config.agent.default_model = "local-chat".to_owned();
    config.agent.fast_model = "local-chat".to_owned();

    assert!(config.validate().is_ok());
    assert!(remove_provider(&mut config, "local").is_err());

    config.models.clear();
    remove_provider(&mut config, "local").unwrap();
    assert!(config.providers.is_empty());
}

#[test]
fn model_config_defaults_to_vision_when_field_is_omitted() {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-model-vision-default-{}.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        r#"
[agent]
default_model = "chat"
fast_model = "chat"

[[providers]]
name = "local"
protocol = "openai"
base_url = "https://provider.example/v1"

[[models]]
name = "chat"
id = "chat"
provider = "local"
context_window = 32000
"#,
    )
    .unwrap();

    let loaded = ConfigLoader::load_from_path(&path).unwrap();

    assert!(loaded.config.models[0].supports_vision);
    assert_eq!(
        loaded.config.models[0].max_output_tokens,
        Some(super::DEFAULT_MODEL_MAX_OUTPUT_TOKENS)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn anthropic_model_allows_omitted_output_tokens_and_rejects_zero() {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "anthropic",
        super::PROVIDER_PROTOCOL_ANTHROPIC,
        "https://api.anthropic.com/v1",
    ));
    config.models.push(ModelConfig {
        name: "claude".to_owned(),
        id: "claude".to_owned(),
        provider: "anthropic".to_owned(),
        context_window: 200_000,
        max_output_tokens: None,
        supports_vision: true,
        supports_embedding: false,
        embedding_dimension: 0,
        requires_reasoning_content_on_assistant_messages: false,
    });
    config.agent.default_model = "claude".to_owned();
    config.agent.fast_model = "claude".to_owned();

    assert!(config.validate().is_ok());

    config.models[0].max_output_tokens = Some(0);
    assert!(config.validate().is_err());
}

#[test]
fn invalid_provider_catalog_fails_closed() {
    let mut config = AxonConfig::default();
    config
        .providers
        .push(ProviderConfig::new("bad", "openai", "file:///not-allowed"));

    assert!(config.validate().is_err());
}

#[test]
fn provider_wire_transport_validates_supported_values() {
    let mut config = AxonConfig::default();
    let mut provider = ProviderConfig::new("realtime", "openai", "https://provider.example/v1");
    provider.wire_transport = super::PROVIDER_WIRE_TRANSPORT_WEBSOCKET.to_owned();
    provider.websocket_url = Some("wss://provider.example/v1/realtime".to_owned());
    config.providers.push(provider);

    assert!(config.validate().is_ok());

    config.providers[0].wire_transport = "socket".to_owned();
    assert!(config.validate().is_err());

    config.providers[0].wire_transport = super::PROVIDER_WIRE_TRANSPORT_WEBSOCKET.to_owned();
    config.providers[0].websocket_url = Some("file:///not-allowed".to_owned());
    assert!(config.validate().is_err());
}

#[test]
fn model_catalog_allows_distinct_aliases_for_same_provider_model_id() {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "openai",
        "openai",
        "https://provider.example/v1",
    ));
    config.models.push(ModelConfig {
        name: "gpt-5.5".to_owned(),
        id: "upstream-gpt-5.5".to_owned(),
        provider: "openai".to_owned(),
        context_window: 128_000,
        ..ModelConfig::default()
    });
    config.models.push(ModelConfig {
        name: "claude-sonnet-4-6".to_owned(),
        id: "upstream-gpt-5.5".to_owned(),
        provider: "openai".to_owned(),
        context_window: 128_000,
        ..ModelConfig::default()
    });
    config.agent.default_model = "gpt-5.5".to_owned();
    config.agent.fast_model = "claude-sonnet-4-6".to_owned();

    assert!(config.validate().is_ok());

    config.models.push(ModelConfig {
        name: "gpt-5.5".to_owned(),
        id: "upstream-gpt-5.5".to_owned(),
        provider: "openai".to_owned(),
        context_window: 128_000,
        ..ModelConfig::default()
    });
    assert!(config.validate().is_err());
}

#[test]
fn memory_embedding_resolves_provider_from_model_catalog() {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "openai",
        "openai",
        "https://api.openai.com/v1",
    ));
    config.models.push(ModelConfig {
        name: "text-embedding-3-small".to_owned(),
        id: "text-embedding-3-small".to_owned(),
        provider: "openai".to_owned(),
        context_window: 0,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: true,
        embedding_dimension: 1_536,
        requires_reasoning_content_on_assistant_messages: false,
    });
    config.agent.default_model = "text-embedding-3-small".to_owned();
    config.agent.fast_model = "text-embedding-3-small".to_owned();
    config.memory.embedding.enabled = true;
    config.memory.embedding.model = "text-embedding-3-small".to_owned();
    config.memory.embedding.dimension = 3_072;

    assert!(config.validate().is_ok());
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-memory-embedding-{}-derived.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    ConfigLoader::save_to_path(&path, &config).unwrap();
    let loaded = ConfigLoader::load_from_path(&path).unwrap();
    assert_eq!(loaded.config.memory.embedding.dimension, 1_536);
    let saved = std::fs::read_to_string(&path).unwrap();
    let memory_embedding_section = saved
        .split("[memory.embedding]")
        .nth(1)
        .and_then(|tail| tail.split("\n[").next())
        .unwrap_or_default();
    assert!(!memory_embedding_section.contains("dimension ="));
    assert!(!memory_embedding_section.contains("provider ="));
    let _ = std::fs::remove_file(path);
}

#[test]
fn memory_embedding_accepts_bailian_and_ark_provider_protocols() {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "bailian",
        "openai-chatcompletions",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
    ));
    config.providers.push(ProviderConfig::new(
        "bailian_vl",
        "dashscope",
        "https://dashscope.aliyuncs.com/api/v1",
    ));
    config.providers.push(ProviderConfig::new(
        "ark",
        "volcengine-ark-multimodal",
        "https://ark.cn-beijing.volces.com/api/v3",
    ));
    config.models.push(ModelConfig {
        name: "text-embedding-v4".to_owned(),
        id: "text-embedding-v4".to_owned(),
        provider: "bailian".to_owned(),
        context_window: 0,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: true,
        embedding_dimension: 1_536,
        requires_reasoning_content_on_assistant_messages: false,
    });
    config.models.push(ModelConfig {
        name: "doubao-embedding-vision-251215".to_owned(),
        id: "doubao-embedding-vision-251215".to_owned(),
        provider: "ark".to_owned(),
        context_window: 0,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: true,
        embedding_dimension: 2_048,
        requires_reasoning_content_on_assistant_messages: false,
    });
    config.models.push(ModelConfig {
        name: "qwen3-vl-embedding".to_owned(),
        id: "qwen3-vl-embedding".to_owned(),
        provider: "bailian_vl".to_owned(),
        context_window: 0,
        max_output_tokens: None,
        supports_vision: true,
        supports_embedding: true,
        embedding_dimension: 1_024,
        requires_reasoning_content_on_assistant_messages: false,
    });
    config.agent.default_model = "text-embedding-v4".to_owned();
    config.agent.fast_model = "text-embedding-v4".to_owned();
    config.memory.embedding.enabled = true;
    config.memory.embedding.model = "text-embedding-v4".to_owned();
    config.memory.embedding.dimension = 1_536;
    assert!(config.validate().is_ok());

    config.memory.embedding.model = "doubao-embedding-vision-251215".to_owned();
    config.memory.embedding.dimension = 2_048;
    assert!(config.validate().is_ok());

    config.memory.embedding.model = "qwen3-vl-embedding".to_owned();
    config.memory.embedding.dimension = 1_024;
    assert!(config.validate().is_ok());
}

#[test]
fn provider_model_protocol_capability_gate_matches_model_kind() {
    let mut chat_on_dashscope_provider = AxonConfig::default();
    chat_on_dashscope_provider
        .providers
        .push(ProviderConfig::new(
            "bailian",
            "dashscope",
            "https://dashscope.aliyuncs.com/api/v1",
        ));
    chat_on_dashscope_provider.models.push(ModelConfig {
        name: "qwen-chat".to_owned(),
        id: "qwen-chat".to_owned(),
        provider: "bailian".to_owned(),
        context_window: 32_000,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: false,
        embedding_dimension: 0,
        requires_reasoning_content_on_assistant_messages: false,
    });
    chat_on_dashscope_provider.agent.default_model = "qwen-chat".to_owned();
    chat_on_dashscope_provider.agent.fast_model = "qwen-chat".to_owned();

    assert!(chat_on_dashscope_provider.validate().is_ok());

    let mut embedding_on_chat_only_provider = AxonConfig::default();
    embedding_on_chat_only_provider
        .providers
        .push(ProviderConfig::new(
            "anthropic",
            "anthropic",
            "https://api.anthropic.com/v1",
        ));
    embedding_on_chat_only_provider.models.push(ModelConfig {
        name: "embedding-on-chat-provider".to_owned(),
        id: "embedding-on-chat-provider".to_owned(),
        provider: "anthropic".to_owned(),
        context_window: 0,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: true,
        embedding_dimension: 1_536,
        requires_reasoning_content_on_assistant_messages: false,
    });
    embedding_on_chat_only_provider.agent.default_model = "embedding-on-chat-provider".to_owned();
    embedding_on_chat_only_provider.agent.fast_model = "embedding-on-chat-provider".to_owned();

    assert!(embedding_on_chat_only_provider.validate().is_err());
}

#[test]
fn provider_capabilities_are_derived_from_protocol_only() {
    let openai = ProviderConfig::new(
        "openai",
        super::PROVIDER_PROTOCOL_OPENAI,
        "https://provider.example/v1",
    );
    let anthropic = ProviderConfig::new(
        "anthropic",
        super::PROVIDER_PROTOCOL_ANTHROPIC,
        "https://provider.example/v1",
    );
    let compatible = ProviderConfig::new(
        "compatible",
        super::PROVIDER_PROTOCOL_OPENAI_CHAT,
        "https://provider.example/v1",
    );

    assert_eq!(
        openai.capabilities(),
        ProviderCapabilities::for_protocol(super::PROVIDER_PROTOCOL_OPENAI)
    );
    assert_eq!(
        anthropic.capabilities(),
        ProviderCapabilities::for_protocol(super::PROVIDER_PROTOCOL_ANTHROPIC)
    );
    assert_eq!(compatible.capabilities(), ProviderCapabilities::stateless());
}

#[test]
fn provider_capabilities_are_not_user_configurable_or_persisted() {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "openai",
        super::PROVIDER_PROTOCOL_OPENAI,
        "https://provider.example/v1",
    ));

    let serialized = to_toml_string(&config).unwrap();
    assert!(!serialized.contains("capabilities"));
    assert!(!serialized.contains("supports_prompt_cache"));
    assert!(!serialized.contains("supports_explicit_cache_breakpoint"));
    assert!(!serialized.contains("supports_server_conversation_state"));

    let path = axon_test_support::test_temp_root().join(format!(
        "axon-provider-capabilities-ignored-{}.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        r#"
[[providers]]
name = "openai"
protocol = "openai"
base_url = "https://provider.example/v1"

[providers.capabilities]
supports_prompt_cache = false
supports_explicit_cache_breakpoint = true
supports_server_conversation_state = false
"#,
    )
    .unwrap();

    // 能力块不是配置契约：未声明字段被忽略，能力仍按协议推导。
    let loaded = ConfigLoader::load_from_path(&path).unwrap();
    let provider = loaded.config.providers.first().expect("provider 已加载");
    assert!(provider.capabilities().supports_prompt_cache);
    assert!(!provider.capabilities().supports_explicit_cache_breakpoint);
    let serialized = to_toml_string(&loaded.config).unwrap();
    assert!(!serialized.contains("capabilities"));
    assert!(!serialized.contains("supports_prompt_cache"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn model_reasoning_content_compat_defaults_off_and_round_trips() {
    let default_model = ModelConfig::default();
    assert!(!default_model.requires_reasoning_content_passback());
    assert!(
        !ProviderConfig::new(
            "thinking-gateway",
            super::PROVIDER_PROTOCOL_OPENAI_CHAT,
            "https://provider.example/v1",
        )
        .capabilities()
        .with_model_reasoning_content_passback(default_model.requires_reasoning_content_passback())
        .requires_reasoning_content_on_assistant_messages
    );

    let path = axon_test_support::test_temp_root().join(format!(
        "axon-model-reasoning-content-compat-{}.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        r#"
[agent]
default_model = "thinking"
fast_model = "thinking"

[[providers]]
name = "thinking-gateway"
protocol = "openai-chatcompletions"
base_url = "https://provider.example/v1"

[[models]]
name = "thinking"
id = "thinking-model"
provider = "thinking-gateway"
context_window = 128000
requires_reasoning_content_on_assistant_messages = true
"#,
    )
    .unwrap();

    let loaded = ConfigLoader::load_from_path(&path).unwrap();
    let model = loaded
        .config
        .models
        .iter()
        .find(|model| model.name == "thinking")
        .expect("model 已加载");
    assert!(model.requires_reasoning_content_passback());

    let serialized = to_toml_string(&loaded.config).unwrap();
    assert!(serialized.contains("requires_reasoning_content_on_assistant_messages = true"));
    let _ = std::fs::remove_file(path);

    let mut config = AxonConfig::default();
    config.agent.default_model = "plain".to_owned();
    config.agent.fast_model = "plain".to_owned();
    config.providers.push(ProviderConfig::new(
        "thinking-gateway",
        super::PROVIDER_PROTOCOL_OPENAI_CHAT,
        "https://provider.example/v1",
    ));
    config.models.push(ModelConfig {
        name: "plain".to_owned(),
        id: "plain-model".to_owned(),
        provider: "thinking-gateway".to_owned(),
        ..ModelConfig::default()
    });
    let serialized = to_toml_string(&config).unwrap();
    assert!(!serialized.contains("requires_reasoning_content_on_assistant_messages"));
}
