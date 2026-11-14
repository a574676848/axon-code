//! 配置路由测试：载荷形状、未命中回落、方法边界、联合模式分派与落盘结论。

use axon_daemon::GatewayHttpRouteHandler;
use serde_json::{Value, json};

use super::{CONFIG_PATH, CONFIG_SCHEMA_PATH, ConfigHttpAdapter};

/// 构造一个指向临时配置路径的适配器。
fn fixture() -> (axon_test_support::TestTempDir, ConfigHttpAdapter) {
    let directory = axon_test_support::test_temp_dir();
    let adapter = ConfigHttpAdapter::new(directory.path().join("config.toml"));
    (directory, adapter)
}

/// 发一次请求并按 JSON 解析回执。
fn call(adapter: &ConfigHttpAdapter, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    let response = adapter
        .handle_request(method, path, None, body)
        .expect("配置路由必须命中");
    let payload = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
    (response.status, payload)
}

/// 发一次读取请求。
fn read_config(adapter: &ConfigHttpAdapter) -> Value {
    let (status, payload) = call(adapter, "GET", CONFIG_PATH, &[]);
    assert_eq!(status, 200);
    payload
}

/// 待写入的合法 TOML：把日志级别改成非默认值，落盘后能观测到差异。
const SAMPLE_TOML: &str = "[agent]\nlog_level = \"info\"\n";

#[test]
fn read_returns_the_desktop_config_read_shape() {
    let (_directory, adapter) = fixture();
    let payload = read_config(&adapter);

    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "config_path",
            "env_vars",
            "flat_values",
            "models",
            "provider_protocols",
            "provider_wire_transports",
            "providers",
            "specs",
            "toml_content",
        ],
        "顶层字段必须与桌面端 ConfigReadResponse 逐字一致"
    );

    assert!(payload["flat_values"].is_object(), "扁平键值必须是对象");
    assert!(payload["specs"].is_array(), "键规格必须是数组");
    assert!(payload["providers"].is_array());
    assert!(payload["models"].is_array());
    assert!(
        payload["provider_protocols"]
            .as_array()
            .is_some_and(|items| items.len() == 5),
        "协议候选值必须恰好五项"
    );
    assert!(
        payload["provider_wire_transports"]
            .as_array()
            .is_some_and(|items| items.len() == 2)
    );
    assert!(
        payload["config_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("config.toml")),
        "回执必须带上权威配置文件路径"
    );

    let first_spec = payload["specs"][0].as_object().expect("规格条目是对象");
    let mut spec_keys = first_spec.keys().map(String::as_str).collect::<Vec<_>>();
    spec_keys.sort_unstable();
    assert_eq!(
        spec_keys,
        vec![
            "default_value",
            "depends_on",
            "description",
            "editable",
            "key",
            "kind",
            "options",
            "setup_module",
        ],
        "规格条目字段必须与桌面端 ConfigKeySpecDto 逐字一致"
    );
    assert!(
        first_spec["kind"]
            .as_str()
            .is_some_and(|kind| matches!(kind, "bool" | "unsigned" | "string" | "string_list")),
        "键类型必须是桌面端四档之一"
    );
}

#[test]
fn schema_route_returns_exactly_the_specs_of_the_read_payload() {
    let (_directory, adapter) = fixture();
    let read = read_config(&adapter);
    let (status, schema) = call(&adapter, "GET", CONFIG_SCHEMA_PATH, &[]);

    assert_eq!(status, 200);
    let mut keys = schema
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, vec!["specs"], "键规格路由只回 specs 一项");
    assert_eq!(schema["specs"], read["specs"], "两档的规格必须逐字一致");
}

#[test]
fn unmatched_paths_and_methods_are_not_swallowed() {
    let (_directory, adapter) = fixture();
    for path in [
        "/api/v1/configs",
        "/api/v1/config/schema/extra",
        "/api/v1/secrets",
        "/api/v1/mcp/config",
    ] {
        assert!(
            adapter.handle_request("GET", path, None, &[]).is_none(),
            "{path} 不属于配置族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405_method_not_allowed() {
    let (_directory, adapter) = fixture();
    for (method, path) in [
        ("POST", CONFIG_PATH),
        ("DELETE", CONFIG_PATH),
        ("PUT", CONFIG_SCHEMA_PATH),
        ("POST", CONFIG_SCHEMA_PATH),
    ] {
        let (status, payload) = call(&adapter, method, path, b"");
        assert_eq!(status, 405, "{method} {path}");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}

#[test]
fn unknown_mode_returns_invalid_mode() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(
        &adapter,
        "PUT",
        CONFIG_PATH,
        br#"{"mode":"replaceEverything"}"#,
    );
    assert_eq!(status, 400);
    assert_eq!(payload, json!({ "error": "invalid_mode" }));
}

#[test]
fn body_that_is_not_json_returns_invalid_json() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, b"mode=text");
    assert_eq!(status, 400);
    assert_eq!(payload, json!({ "error": "invalid_json" }));
}

#[test]
fn text_mode_writes_canonical_config_and_skips_an_identical_second_write() {
    let (directory, adapter) = fixture();
    let body = json!({ "mode": "text", "tomlContent": SAMPLE_TOML }).to_string();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, body.as_bytes());

    assert_eq!(status, 200);
    assert_eq!(payload["written"], true);
    assert_eq!(payload["backup_created"], false, "首次写入没有旧文件可备份");
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );

    let written =
        std::fs::read_to_string(directory.path().join("config.toml")).expect("配置必须已落盘");
    assert!(written.contains("log_level = \"info\""), "{written}");
    assert_eq!(
        read_config(&adapter)["flat_values"]["agent.log_level"],
        "info"
    );

    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, body.as_bytes());
    assert_eq!(status, 200);
    assert_eq!(payload["written"], false, "内容未变时必须跳过写入");
    assert_eq!(payload["backup_created"], false);
}

#[test]
fn invalid_toml_is_rejected_before_the_file_is_created() {
    let (directory, adapter) = fixture();
    let body = json!({ "mode": "text", "tomlContent": "这不是 [[ TOML" }).to_string();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, body.as_bytes());

    assert_eq!(status, 400);
    assert_eq!(payload["error"], "config_invalid");
    assert!(payload["message"].is_string(), "校验失败必须带上中文原因");
    assert!(
        !directory.path().join("config.toml").exists(),
        "校验不通过时绝不能落盘"
    );
}

#[test]
fn values_mode_applies_keys_and_persists_them() {
    let (directory, adapter) = fixture();
    let body = json!({
        "mode": "values",
        "values": { "agent.log_level": "debug" },
    })
    .to_string();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, body.as_bytes());

    assert_eq!(status, 200);
    assert_eq!(payload["written"], true);
    assert!(directory.path().join("config.toml").exists());
    assert_eq!(
        read_config(&adapter)["flat_values"]["agent.log_level"],
        "debug"
    );
}

#[test]
fn values_mode_rejects_a_non_string_value() {
    let (_directory, adapter) = fixture();
    let body = json!({
        "mode": "values",
        "values": { "agent.max_subagent_threads": 16 },
    })
    .to_string();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, body.as_bytes());

    assert_eq!(status, 400);
    assert_eq!(payload["error"], "invalid_values");
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|message| message.contains("agent.max_subagent_threads"))
    );
}

#[test]
fn values_mode_reports_no_change_for_an_empty_map() {
    let (_directory, adapter) = fixture();
    let body = json!({ "mode": "values", "values": {} }).to_string();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, body.as_bytes());

    assert_eq!(status, 200);
    assert_eq!(payload["written"], false);
    assert_eq!(payload["backup_created"], false);
}

#[test]
fn reset_key_restores_the_builtin_default() {
    let (directory, adapter) = fixture();
    let body = json!({
        "mode": "values",
        "values": { "agent.log_level": "debug" },
    })
    .to_string();
    assert_eq!(call(&adapter, "PUT", CONFIG_PATH, body.as_bytes()).0, 200);

    let reset = json!({ "mode": "resetKey", "key": "agent.log_level" }).to_string();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, reset.as_bytes());
    assert_eq!(status, 200);
    assert_eq!(payload["written"], true);
    assert!(
        directory.path().join("config.toml").exists(),
        "重置同样要落盘"
    );

    let default_value =
        axon_config::get_config_value(&axon_config::AxonConfig::default(), "agent.log_level")
            .expect("内置默认值必须可读")
            .to_string();
    assert_eq!(
        read_config(&adapter)["flat_values"]["agent.log_level"],
        default_value
    );
}

#[test]
fn reset_key_rejects_an_unknown_key() {
    let (_directory, adapter) = fixture();
    let reset = json!({ "mode": "resetKey", "key": "agent.not_a_key" }).to_string();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, reset.as_bytes());

    assert_eq!(status, 400);
    assert_eq!(payload["error"], "unknown_key");
}

#[test]
fn validate_mode_never_touches_the_disk() {
    let (directory, adapter) = fixture();

    let broken = json!({ "mode": "validate", "tomlContent": "这不是 [[ TOML" }).to_string();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, broken.as_bytes());
    assert_eq!(status, 200, "校验结论走 200，是否通过看 valid");
    assert_eq!(payload["valid"], false);
    assert!(
        payload["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
    );
    assert_eq!(payload["warnings"], json!([]));
    assert_eq!(payload["diff_summary"], json!([]));

    let valid = json!({ "mode": "validate", "tomlContent": SAMPLE_TOML }).to_string();
    let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, valid.as_bytes());
    assert_eq!(status, 200);
    assert_eq!(payload["valid"], true);
    assert_eq!(payload["errors"], json!([]));
    assert!(
        payload["diff_summary"]
            .as_array()
            .is_some_and(|summary| !summary.is_empty())
    );

    assert!(
        !directory.path().join("config.toml").exists(),
        "校验模式绝不能落盘"
    );
}

#[test]
fn missing_mode_payloads_are_reported_field_by_field() {
    let (_directory, adapter) = fixture();
    for (body, expected) in [
        (
            json!({ "mode": "text" }).to_string(),
            "missing_toml_content",
        ),
        (
            json!({ "mode": "validate" }).to_string(),
            "missing_toml_content",
        ),
        (json!({ "mode": "values" }).to_string(), "missing_values"),
        (json!({ "mode": "resetKey" }).to_string(), "missing_key"),
    ] {
        let (status, payload) = call(&adapter, "PUT", CONFIG_PATH, body.as_bytes());
        assert_eq!(status, 400, "{body}");
        assert_eq!(payload["error"], expected, "{body}");
    }
}

#[test]
fn read_projects_providers_and_models_from_the_loaded_config() {
    let (directory, adapter) = fixture();
    let mut config = axon_config::AxonConfig::default();
    let mut provider = axon_config::ProviderConfig::new(
        "fixture",
        axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        "https://api.example.com/v1",
    );
    provider.api_key_env = Some("FIXTURE_API_KEY".to_owned());
    config.providers.push(provider);
    config.models.push(axon_config::ModelConfig {
        name: "fixture-model".to_owned(),
        id: "fixture-model-id".to_owned(),
        provider: "fixture".to_owned(),
        ..axon_config::ModelConfig::default()
    });
    // 有模型目录时两个模型槽位必须指向已配置模型，否则配置校验不通过。
    config.agent.default_model = "fixture-model".to_owned();
    config.agent.fast_model = "fixture-model".to_owned();
    axon_config::ConfigLoader::save_to_path(&directory.path().join("config.toml"), &config)
        .expect("测试配置必须可写");

    let payload = read_config(&adapter);
    let providers = payload["providers"].as_array().expect("供应商是数组");
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["name"], "fixture");
    assert_eq!(providers[0]["protocol"], "anthropic");
    assert_eq!(providers[0]["api_key_env"], "FIXTURE_API_KEY");
    assert_eq!(
        providers[0]["api_key_configured"], false,
        "没有 .env 时密钥必须报未配置"
    );

    let models = payload["models"].as_array().expect("模型是数组");
    assert_eq!(models.len(), 1);
    let mut model_keys = models[0]
        .as_object()
        .expect("模型条目是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    model_keys.sort_unstable();
    assert_eq!(
        model_keys,
        vec![
            "context_window",
            "embedding_dimension",
            "id",
            "max_output_tokens",
            "name",
            "provider",
            "requires_reasoning_content_on_assistant_messages",
            "supports_embedding",
            "supports_vision",
        ],
        "模型字段必须与桌面端 ModelDto 逐字一致"
    );
}
