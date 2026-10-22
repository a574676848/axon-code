use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use axon_config::{AxonConfig, ConfigLoader, ModelConfig, ProviderConfig};
use serde_json::Value;

use super::{AxonCli, run_parsed_cli};

#[test]
fn setup_non_interactive_json_writes_user_level_config_env_and_report() {
    let root = temp_dir("setup-json");
    let config_path = root.join("config.toml");
    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
        "--format",
        "json",
        "--provider-name",
        "wenrugouai-a",
        "--protocol",
        "openai",
        "--base-url",
        "https://api.wenrugouai.example/v1",
        "--wire-transport",
        "websocket",
        "--websocket-url",
        "wss://api.wenrugouai.example/ws",
        "--model",
        "gpt-5.5",
        "--model-name",
        "gpt-5.5-alias",
        "--reasoning-effort",
        "high",
        "--context-window",
        "200000",
        "--api-key-env",
        "WENRUGOUAI_A_API_KEY",
        "--api-key",
        "secret-value",
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let report: Value = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(report["providerName"], "wenrugouai-a");
    assert_eq!(report["modelId"], "gpt-5.5");
    assert_eq!(report["credentialStatus"], "present");
    let loaded = ConfigLoader::load_from_path(&config_path).unwrap();
    let provider = loaded
        .config
        .providers
        .iter()
        .find(|provider| provider.name == "wenrugouai-a")
        .unwrap();
    assert_eq!(provider.wire_transport, "websocket");
    assert_eq!(
        provider.websocket_url.as_deref(),
        Some("wss://api.wenrugouai.example/ws")
    );
    let model = loaded
        .config
        .models
        .iter()
        .find(|model| model.provider == "wenrugouai-a" && model.id == "gpt-5.5")
        .unwrap();
    assert_eq!(model.name, "gpt-5.5-alias");
    assert_eq!(loaded.config.agent.default_model, "gpt-5.5-alias");
    assert_eq!(loaded.config.agent.fast_model, "gpt-5.5-alias");
    assert_eq!(loaded.config.agent.reasoning_effort, "high");
    assert!(
        fs::read_to_string(root.join(".env"))
            .unwrap()
            .contains("WENRUGOUAI_A_API_KEY=secret-value")
    );
    let env_text = fs::read_to_string(root.join(".env")).unwrap();
    assert!(env_text.contains("WENRUGOUAI_A_API_KEY=secret-value"));
    let gateway_token = env_text
        .lines()
        .find_map(|line| line.strip_prefix("AXON_GATEWAY_API_TOKEN="))
        .unwrap();
    assert!(!result.stdout.contains(gateway_token));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_quick_preserves_existing_provider_model_and_overwrites_explicit_secret() {
    let root = temp_dir("setup-quick");
    let config_path = root.join("config.toml");
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig {
        name: "existing".to_owned(),
        enabled: true,
        protocol: "openai".to_owned(),
        base_url: "https://old.example/v1".to_owned(),
        api_key_env: Some("EXISTING_API_KEY".to_owned()),
        api_key: None,
        wire_transport: "http".to_owned(),
        websocket_url: None,
        enable_request_compression: false,
        priority: 0,
        weight: 1,
    });
    config.models.push(ModelConfig {
        name: "alias".to_owned(),
        id: "model-a".to_owned(),
        provider: "existing".to_owned(),
        context_window: 32_000,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: false,
        embedding_dimension: 0,
    });
    config.agent.default_model = "alias".to_owned();
    config.agent.fast_model = "alias".to_owned();
    ConfigLoader::save_to_path(&config_path, &config).unwrap();
    fs::write(root.join(".env"), "EXISTING_API_KEY=keep\n").unwrap();

    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
        "--quick",
        "--provider-name",
        "existing",
        "--base-url",
        "https://new.example/v1",
        "--model",
        "model-a",
        "--api-key-env",
        "EXISTING_API_KEY",
        "--api-key",
        "replace",
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let loaded = ConfigLoader::load_from_path(&config_path).unwrap();
    assert_eq!(
        loaded.config.providers[0].base_url,
        "https://old.example/v1"
    );
    assert_eq!(loaded.config.agent.default_model, "alias");
    assert_eq!(loaded.config.agent.fast_model, "alias");
    let env_text = fs::read_to_string(root.join(".env")).unwrap();
    assert!(env_text.starts_with("EXISTING_API_KEY=replace\n"));
    assert!(env_text.contains("AXON_GATEWAY_API_TOKEN="));
    assert!(result.stdout.contains("provider.skipped"));
    assert!(result.stdout.contains("model.skipped"));
    assert!(result.stdout.contains("secret.updated"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_agent_config_drives_non_interactive_configuration() {
    let root = temp_dir("setup-agent-config");
    fs::create_dir_all(&root).unwrap();
    let config_path = root.join("config.toml");
    let agent_config_path = root.join("setup.json");
    fs::write(
        &agent_config_path,
        r#"{
  "providerName": "agent-provider",
  "protocol": "anthropic",
  "baseUrl": "https://api.anthropic.com",
  "model": "claude-opus-4-8",
  "modelName": "opus-strong",
  "reasoningEffort": "high",
  "contextWindow": 200000,
  "apiKeyEnv": "ANTHROPIC_API_KEY",
  "configValues": {}
}"#,
    )
    .unwrap();

    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
        "--agent-config",
        agent_config_path.to_str().unwrap(),
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let loaded = ConfigLoader::load_from_path(&config_path).unwrap();
    assert_eq!(loaded.config.providers[0].name, "agent-provider");
    assert_eq!(loaded.config.providers[0].protocol, "anthropic");
    assert_eq!(loaded.config.models[0].id, "claude-opus-4-8");
    assert_eq!(loaded.config.models[0].name, "opus-strong");
    assert_eq!(
        loaded.config.models[0].max_output_tokens,
        Some(axon_config::DEFAULT_MODEL_MAX_OUTPUT_TOKENS)
    );
    assert_eq!(loaded.config.agent.default_model, "opus-strong");
    assert_eq!(loaded.config.agent.fast_model, "opus-strong");
    assert_eq!(loaded.config.agent.reasoning_effort, "high");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_agent_config_supports_empty_and_linked_config_values() {
    let root = temp_dir("setup-empty-linked-config");
    fs::create_dir_all(&root).unwrap();
    let config_path = root.join("config.toml");
    let agent_config_path = root.join("setup.json");
    fs::write(
        &agent_config_path,
        r#"{
  "providerName": "remote-provider",
  "baseUrl": "https://api.example.com/v1",
  "model": "gpt-4o",
  "configValues": {
    "remote.feishu_websocket.enabled": "true",
    "remote.feishu_websocket.app_id": "",
    "remote.feishu_websocket.app_secret_env": "",
    "remote.feishu_websocket.allowed_sender_open_ids": "",
    "remote.feishu_websocket.blocked_sender_open_ids": "",
    "remote.mode": "high-throughput"
  }
}"#,
    )
    .unwrap();

    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
        "--agent-config",
        agent_config_path.to_str().unwrap(),
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let loaded = ConfigLoader::load_from_path(&config_path).unwrap();
    assert!(loaded.config.remote.feishu_websocket.enabled);
    assert!(loaded.config.remote.feishu_websocket.app_id.is_empty());
    assert!(
        loaded
            .config
            .remote
            .feishu_websocket
            .allowed_sender_open_ids
            .is_empty()
    );
    assert_eq!(loaded.config.remote.mode.as_str(), "high-throughput");
    assert!(root.join(".env").is_file());
    let env_text = fs::read_to_string(root.join(".env")).unwrap();
    assert!(env_text.contains("AXON_GATEWAY_API_TOKEN="));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn config_keys_json_exposes_setup_choices_for_linked_fields() {
    let result = run_setup(["axon", "--format", "json", "config", "keys"]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let keys: Value = serde_json::from_str(&result.stdout).unwrap();
    let specs = keys.as_array().unwrap();
    assert_eq!(find_key(specs, "remote.mode")["setup_module"], "remote");
    let wire_transport = find_key(specs, "remote.feishu_websocket.enabled");
    assert_eq!(wire_transport["setup_module"], "remote");
    assert_eq!(wire_transport["default_value"], "false");
    assert_eq!(
        wire_transport["options"],
        serde_json::json!(["true", "false"])
    );
    assert_eq!(
        wire_transport["depends_on"],
        serde_json::json!([])
    );
    assert!(
        specs
            .iter()
            .all(|spec| spec["key"] != "default_model_policy")
    );
    for key in [
        "remote.allow_stdin",
        "vision.max_images_per_turn",
        "memory.embedding.dimension",
        "attachment.max_file_bytes",
        "attachment.max_inline_preview_bytes",
        "attachment.allowed_mime_types",
        "attachment.blob_ttl_ms",
        "attachment.remote_upload_default",
    ] {
        assert!(
            specs.iter().all(|spec| spec["key"] != key),
            "固定或派生配置不应出现在 config keys: {key}"
        );
    }
}

#[test]
fn setup_secret_only_updates_env_without_creating_config_toml() {
    let root = temp_dir("setup-secret-only");
    let config_path = root.join("config.toml");
    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
        "--format",
        "json",
        "--api-key",
        "my-secret",
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let report: Value = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(report["status"], "ok");
    assert_eq!(report["credentialStatus"], "present");
    assert!(!config_path.exists(), "Secret-only 不创建 config.toml");
    assert!(
        fs::read_to_string(root.join(".env"))
            .unwrap()
            .contains("OPENAI_API_KEY=my-secret")
    );
    assert!(!result.stdout.contains("my-secret"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_quick_with_no_mutation_creates_no_files() {
    let root = temp_dir("setup-quick-no-mutation");
    let config_path = root.join("config.toml");
    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
        "--quick",
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert!(!config_path.exists(), "Quick 无显式字段不创建 config.toml");
    assert!(root.join(".env").is_file());
    let env_text = fs::read_to_string(root.join(".env")).unwrap();
    assert!(env_text.contains("AXON_GATEWAY_API_TOKEN="));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_non_interactive_without_explicit_input_preserves_existing_config_bytes() {
    let root = temp_dir("setup-no-mutation");
    fs::create_dir_all(&root).unwrap();
    let config_path = root.join("config.toml");
    let original = "# 用户注释必须保留\n[remote]\n".as_bytes();
    fs::write(&config_path, original).unwrap();

    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(fs::read(&config_path).unwrap(), original);
    let env_text = fs::read_to_string(root.join(".env")).unwrap();
    assert!(env_text.contains("AXON_GATEWAY_API_TOKEN="));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_cli_secret_has_priority_over_agent_config_clear() {
    let root = temp_dir("setup-cli-priority");
    fs::create_dir_all(&root).unwrap();
    let config_path = root.join("config.toml");
    let agent_config_path = root.join("setup.json");
    fs::write(&agent_config_path, r#"{"clearFields":["apiKey"]}"#).unwrap();

    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
        "--agent-config",
        agent_config_path.to_str().unwrap(),
        "--api-key",
        "cli-secret",
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let env_text = fs::read_to_string(root.join(".env")).unwrap();
    assert!(env_text.starts_with("OPENAI_API_KEY=cli-secret\n"));
    assert!(env_text.contains("AXON_GATEWAY_API_TOKEN="));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_reset_provider_removes_provider_and_models() {
    let root = temp_dir("setup-reset-provider");
    let config_path = root.join("config.toml");
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig {
        name: "target-p".to_owned(),
        enabled: true,
        protocol: "openai".to_owned(),
        base_url: "https://target.example/v1".to_owned(),
        api_key_env: Some("TARGET_KEY".to_owned()),
        api_key: None,
        wire_transport: "http".to_owned(),
        websocket_url: None,
        enable_request_compression: false,
        priority: 0,
        weight: 1,
    });
    config.models.push(ModelConfig {
        name: "target-m".to_owned(),
        id: "target-m".to_owned(),
        provider: "target-p".to_owned(),
        context_window: 128_000,
        max_output_tokens: None,
        supports_vision: true,
        supports_embedding: false,
        embedding_dimension: 0,
    });
    config.agent.default_model = "target-m".to_owned();
    config.agent.fast_model = "target-m".to_owned();
    ConfigLoader::save_to_path(&config_path, &config).unwrap();

    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
        "--reset-provider",
        "target-p",
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let loaded = ConfigLoader::load_from_path(&config_path).unwrap();
    assert!(loaded.config.providers.is_empty());
    assert!(loaded.config.models.is_empty());
    assert!(loaded.config.agent.default_model.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_json_error_output_contains_file_states() {
    let root = temp_dir("setup-json-error");
    let config_path = root.join("config.toml");
    let agent_config_path = root.join("setup.json");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &agent_config_path,
        r#"{"configValues": {"agent.default_model": "invalid"}}"#,
    )
    .unwrap();

    let result = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
        "--format",
        "json",
        "--agent-config",
        agent_config_path.to_str().unwrap(),
    ]);

    assert_eq!(result.exit_code, 2);
    let err_json: Value = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(err_json["outcome"], "error");
    assert!(err_json["message"].as_str().unwrap().contains("configValues 包含已被结构化 Setup 字段表示的 key"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_gateway_token_is_displayed_once_and_remains_idempotent() {
    let root = temp_dir("setup-gateway-token");
    let config_path = root.join("config.toml");
    let first = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
    ]);
    assert_eq!(first.exit_code, 0, "{}", first.stderr);
    let env_text = fs::read_to_string(root.join(".env")).unwrap();
    let token = env_text
        .lines()
        .find_map(|line| line.strip_prefix("AXON_GATEWAY_API_TOKEN="))
        .unwrap()
        .to_owned();
    assert_eq!(token.len(), 64);
    assert!(first.stdout.contains(&token));

    let second = run_setup([
        "axon",
        "--config",
        config_path.to_str().unwrap(),
        "setup",
        "--non-interactive",
    ]);
    assert_eq!(second.exit_code, 0, "{}", second.stderr);
    assert!(!second.stdout.contains(&token));
    assert_eq!(
        fs::read_to_string(root.join(".env")).unwrap(),
        env_text
    );
    let _ = fs::remove_dir_all(root);
}

fn run_setup<const N: usize>(args: [&str; N]) -> super::CliRunResult {
    let cli = AxonCli::parse_from_args(args);
    run_parsed_cli(&cli)
}

fn find_key<'a>(specs: &'a [Value], key: &str) -> &'a Value {
    specs
        .iter()
        .find(|spec| spec["key"] == key)
        .unwrap_or_else(|| panic!("缺少配置键 {key}"))
}

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    axon_test_support::test_temp_root().join(format!("axon-{label}-{}-{nonce}", std::process::id()))
}
