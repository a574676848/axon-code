use super::{
    AxonConfig, ConfigLoader, ConfigValue, HIGH_THROUGHPUT_REMOTE_MAX_EVENTS_PER_CURSOR,
    HIGH_THROUGHPUT_REMOTE_MAX_PAYLOAD_BYTES, ModelConfig, REASONING_EFFORT_VALUES, RemoteMode,
    STANDARD_REMOTE_MAX_EVENTS_PER_CURSOR, STANDARD_REMOTE_MAX_PAYLOAD_BYTES, USER_AXON_DIR,
    USER_CONFIG_FILE, config_key_specs, default_user_axon_root_path_from_env, get_config_value,
    set_config_value,
};

#[test]
fn log_level_warning_alias_is_canonicalized_without_duplicate_options() {
    let mut config = AxonConfig::default();
    set_config_value(&mut config, "agent.log_level", "warning").unwrap();
    assert_eq!(config.agent.log_level, "warn");
    let spec = config_key_specs()
        .iter()
        .find(|spec| spec.key == "agent.log_level")
        .unwrap();
    assert_eq!(spec.options, &["trace", "debug", "info", "warn", "error"]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    config.agent.log_level = "warning".to_owned();
    ConfigLoader::save_to_path(&path, &config).unwrap();
    assert_eq!(
        ConfigLoader::load_from_path(&path)
            .unwrap()
            .config
            .agent
            .log_level,
        "warn"
    );
}

#[test]
fn remote_feishu_chat_policies_validate_group_overrides() {
    let mut config = AxonConfig::default();
    config.remote.feishu_websocket.chat_policies =
        vec![super::sections::RemoteFeishuChatPolicyConfig {
            chat_id: "oc_ops".to_owned(),
            allowed_sender_open_ids: vec!["ou_ops".to_owned()],
            blocked_sender_open_ids: Vec::new(),
            require_mention: Some(true),
        }];

    assert!(config.validate().is_ok());
    config.remote.feishu_websocket.chat_policies[0]
        .chat_id
        .clear();
    assert!(config.validate().is_err());
}

#[test]
fn default_loader_records_config_sources() {
    let loaded = ConfigLoader::load_defaults().unwrap();

    assert!(loaded.config.validate().is_ok());
    assert!(loaded.report.ensure_no_secret_plaintext().is_ok());
    assert!(
        loaded
            .report
            .entries
            .iter()
            .all(|entry| entry.key != "goals")
    );
}

#[test]
fn config_accepts_official_reasoning_effort_union_only() {
    for effort in REASONING_EFFORT_VALUES {
        let mut config = AxonConfig::default();
        config.agent.reasoning_effort = (*effort).to_owned();
        assert!(config.validate().is_ok(), "应接受 {effort}");
    }
    for effort in ["ultra", "think-high", "maximum"] {
        let mut config = AxonConfig::default();
        config.agent.reasoning_effort = effort.to_owned();
        assert!(config.validate().is_err(), "应拒绝 {effort}");
    }
}

#[test]
fn root_config_example_toml_is_loadable() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let example_path = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crate 应位于 workspace/crates/axon-config")
        .join("config.example.toml");

    /*
     * 先确认示例文件真的在，再解析。
     *
     * `ConfigLoader::load_from_path` 对不存在的路径回落到 `AxonConfig::default()`，
     * 于是文件缺失只会表现成「default_model 是空串」这种看不出所以然的断言失败。
     * 示例文件是仓库资产，缺失只可能是并发改动（git 操作、备份/索引进程）或真的被删：
     * 短暂缺失先重试一次，仍然不在就带着确切路径失败。
     */
    let mut attempts = 0;
    while !example_path.is_file() && attempts < 2 {
        attempts += 1;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        example_path.is_file(),
        "仓库根的 config.example.toml 缺失：{}",
        example_path.display()
    );

    let loaded = ConfigLoader::load_from_path(&example_path).unwrap();

    assert_eq!(loaded.config.agent.default_model, "gpt-4o-coding");
    assert!(loaded.config.validate().is_ok());
}

#[test]
fn invalid_limits_fail_closed() {
    let mut config = AxonConfig::default();
    config.remote.feishu_websocket.chat_policies =
        vec![super::sections::RemoteFeishuChatPolicyConfig {
            chat_id: String::new(),
            allowed_sender_open_ids: Vec::new(),
            blocked_sender_open_ids: Vec::new(),
            require_mention: None,
        }];

    assert!(config.validate().is_err());

    let mut config = AxonConfig::default();
    config.agent.max_subagent_threads = 0;
    assert!(config.validate().is_err());

    let mut config = AxonConfig::default();
    config.agent.max_subagent_active_turns = 0;
    assert!(config.validate().is_err());

    let mut config = AxonConfig::default();
    config.agent.max_subagent_depth = 0;
    assert!(config.validate().is_err());
}

#[test]
fn config_key_registry_exposes_editability() {
    let specs = config_key_specs();
    let editable_keys = [
        "agent.max_subagent_threads",
        "agent.max_subagent_active_turns",
        "agent.max_subagent_depth",
        "channel_balancer.failure_threshold",
        "channel_balancer.cooldown_ms",
        "channel_balancer.affinity_ttl_ms",
        "memory.embedding.enabled",
        "memory.embedding.model",
        "remote.gateway.api_token_env",
        "remote.gateway.http_bind",
        "remote.gateway.websocket_bind",
        "remote.mode",
    ];

    for key in editable_keys {
        assert_config_key_editability(specs, key, true);
    }
    let memory_keys: Vec<&str> = specs
        .iter()
        .filter_map(|spec| spec.key.strip_prefix("memory.").map(|_| spec.key))
        .collect();
    assert_eq!(
        memory_keys,
        ["memory.embedding.enabled", "memory.embedding.model"]
    );
    assert_target_config_keys_removed(specs);
    assert!(specs.iter().all(|spec| !spec.key.starts_with("goals.")));
    assert!(specs.iter().all(|spec| !spec.key.starts_with("lsp.")));
    assert!(
        specs
            .iter()
            .all(|spec| !spec.key.starts_with("apply_edit."))
    );
    assert!(
        specs
            .iter()
            .all(|spec| !spec.key.starts_with("attachment."))
    );
    assert!(specs.iter().all(|spec| spec.key != "memory.enabled"));
    assert!(specs.iter().all(|spec| !spec.key.starts_with("terminal.")));
    assert!(
        specs
            .iter()
            .all(|spec| spec.key != "apply_edit.require_fresh_diagnostics")
    );
    assert!(
        specs
            .iter()
            .all(|spec| spec.key != "apply_edit.post_apply_verify_diagnostics")
    );
    assert!(
        specs
            .iter()
            .all(|spec| spec.key != "channel_balancer.enable_failover")
    );
}

fn assert_config_key_editability(specs: &[super::ConfigKeySpec], key: &str, editable: bool) {
    assert!(
        specs
            .iter()
            .any(|spec| spec.key == key && spec.editable == editable),
        "配置键 {key} 的 editable 应为 {editable}"
    );
}

#[test]
fn rejects_unknown_config_keys() {
    let mut config = AxonConfig::default();

    assert!(set_config_value(&mut config, "unknown_namespace.unknown_key", "12000").is_err());
}

#[test]
fn agent_config_values_roundtrip_and_validate_bounds() {
    let mut config = AxonConfig::default();

    let value = set_config_value(&mut config, "agent.max_subagent_threads", "4").unwrap();
    assert_eq!(value, ConfigValue::Unsigned(4));
    assert_eq!(config.agent.max_subagent_threads, 4);

    let value = set_config_value(&mut config, "agent.max_subagent_active_turns", "2").unwrap();
    assert_eq!(value, ConfigValue::Unsigned(2));
    assert_eq!(config.agent.max_subagent_active_turns, 2);

    let value = set_config_value(&mut config, "agent.max_subagent_depth", "2").unwrap();
    assert_eq!(value, ConfigValue::Unsigned(2));
    assert_eq!(config.agent.max_subagent_depth, 2);

    assert_eq!(
        get_config_value(&config, "agent.max_subagent_threads"),
        Some(ConfigValue::Unsigned(4))
    );
    assert_eq!(
        get_config_value(&config, "agent.max_subagent_active_turns"),
        Some(ConfigValue::Unsigned(2))
    );
    assert_eq!(
        get_config_value(&config, "agent.max_subagent_depth"),
        Some(ConfigValue::Unsigned(2))
    );

    assert!(set_config_value(&mut config, "agent.max_subagent_threads", "0").is_err());
    assert!(set_config_value(&mut config, "agent.max_subagent_active_turns", "0").is_err());
    assert!(set_config_value(&mut config, "agent.max_subagent_depth", "0").is_err());
}

/// agent.log_level 往返：大小写归一小写，非法级别拒绝。
#[test]
fn agent_log_level_roundtrip_and_rejects_unknown_values() {
    let mut config = AxonConfig::default();

    let value = set_config_value(&mut config, "agent.log_level", " INFO ").unwrap();
    assert_eq!(value, ConfigValue::String("info".to_owned()));
    assert_eq!(config.agent.log_level, "info");
    assert_eq!(
        get_config_value(&config, "agent.log_level"),
        Some(ConfigValue::String("info".to_owned()))
    );

    set_config_value(&mut config, "agent.log_level", "WARN").unwrap();
    assert_eq!(config.agent.log_level, "warn");

    assert!(set_config_value(&mut config, "agent.log_level", "verbose").is_err());
    assert!(set_config_value(&mut config, "agent.log_level", "").is_err());
}

#[test]
fn memory_and_channel_balancer_config_values_roundtrip() {
    let mut config = AxonConfig::default();

    let value = set_config_value(&mut config, "channel_balancer.failure_threshold", "5").unwrap();
    assert_eq!(value, ConfigValue::Unsigned(5));
    assert_eq!(config.channel_balancer.failure_threshold, 5);

    let value = set_config_value(&mut config, "channel_balancer.cooldown_ms", "45000").unwrap();
    assert_eq!(value, ConfigValue::Unsigned(45_000));
    assert_eq!(config.channel_balancer.cooldown_ms, 45_000);
    assert!(set_config_value(&mut config, "channel_balancer.cooldown_ms", "0").is_err());

    assert_eq!(
        set_config_value(&mut config, "memory.embedding.enabled", "false").unwrap(),
        ConfigValue::Bool(false)
    );
    assert_eq!(
        get_config_value(&config, "memory.embedding.enabled"),
        Some(ConfigValue::Bool(false))
    );
}

#[test]
fn removed_runtime_config_keys_are_not_user_editable() {
    let mut config = AxonConfig::default();

    assert!(set_config_value(&mut config, "channel_balancer.enable_failover", "false").is_err());
    assert_eq!(
        get_config_value(&config, "channel_balancer.enable_failover"),
        None
    );
    assert!(set_config_value(&mut config, "memory.enabled", "false").is_err());
    assert_eq!(get_config_value(&config, "memory.enabled"), None);
    assert!(set_config_value(&mut config, "memory.concurrency", "12").is_err());
    assert_eq!(get_config_value(&config, "memory.concurrency"), None);
    assert_eq!(get_config_value(&config, "memory.internal_limit"), None);
    assert!(set_config_value(&mut config, "memory.internal_limit", "8").is_err());
}

#[test]
fn remote_mode_config_value_roundtrips() {
    let mut config = AxonConfig::default();

    let value = set_config_value(
        &mut config,
        "remote.mode",
        RemoteMode::HighThroughput.as_str(),
    )
    .unwrap();
    assert_eq!(
        value,
        ConfigValue::String(RemoteMode::HighThroughput.as_str().to_owned())
    );
    assert_eq!(
        get_config_value(&config, "remote.mode"),
        Some(ConfigValue::String(
            RemoteMode::HighThroughput.as_str().to_owned()
        ))
    );
}

#[test]
fn gateway_listener_addresses_roundtrip_and_validate() {
    let mut config = AxonConfig::default();
    set_config_value(&mut config, "remote.gateway.http_bind", "0.0.0.0:3143").unwrap();
    set_config_value(&mut config, "remote.gateway.websocket_bind", "[::]:3144").unwrap();
    assert_eq!(
        get_config_value(&config, "remote.gateway.http_bind"),
        Some(ConfigValue::String("0.0.0.0:3143".to_owned()))
    );
    assert_eq!(
        get_config_value(&config, "remote.gateway.websocket_bind"),
        Some(ConfigValue::String("[::]:3144".to_owned()))
    );
    assert!(config.validate().is_ok());
}

#[test]
fn internal_lsp_and_diagnostics_keys_are_not_user_editable() {
    let mut config = AxonConfig::default();

    for key in [
        "lsp.read_file_mode",
        "apply_edit.require_fresh_diagnostics",
        "apply_edit.post_apply_verify_diagnostics",
        "apply_edit.require_pre_hash",
        "apply_edit.require_diff_complete",
    ] {
        assert_eq!(get_config_value(&config, key), None);
        assert!(set_config_value(&mut config, key, "true").is_err());
    }

    let toml = super::to_toml_string(&config).unwrap();
    assert!(!toml.contains("[lsp]"));
    assert!(!toml.contains("[apply_edit]"));
    assert!(!toml.contains("require_fresh_diagnostics"));
    assert!(!toml.contains("post_apply_verify_diagnostics"));
    assert!(!toml.contains("require_pre_hash"));
    assert!(!toml.contains("require_diff_complete"));
    assert!(!toml.contains("enable_failover"));
    assert!(!toml.contains("memory.enabled"));
}

#[test]
fn goal_config_namespace_is_absent() {
    let mut config = AxonConfig::default();

    assert_eq!(get_config_value(&config, "goals.workflow_budget"), None);
    assert!(set_config_value(&mut config, "goals.workflow_budget", "1").is_err());

    let toml = super::to_toml_string(&config).unwrap();
    assert!(!toml.contains("[goals]"));
}

#[test]
fn attachment_config_keys_are_not_user_editable() {
    let mut config = AxonConfig::default();

    for key in [
        "attachment.max_file_bytes",
        "attachment.max_inline_preview_bytes",
        "attachment.allowed_mime_types",
        "attachment.blob_ttl_ms",
        "attachment.default_prompt_policy",
        "attachment.remote_upload_default",
    ] {
        assert_eq!(get_config_value(&config, key), None);
        assert!(set_config_value(&mut config, key, "1").is_err());
    }

    let toml = super::to_toml_string(&config).unwrap();
    assert!(!toml.contains("[attachment]"));
    assert!(!toml.contains("max_file_bytes"));
    assert!(!toml.contains("max_inline_preview_bytes"));
    assert!(!toml.contains("allowed_mime_types"));
    assert!(!toml.contains("blob_ttl_ms"));
}

#[path = "persistence_tests.rs"]
mod persistence_tests;
fn config_backup_count(root: &std::path::Path) -> usize {
    std::fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("config.toml.backup-")
        })
        .count()
}

#[test]
fn unknown_memory_pipeline_fields_fail_closed() {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-config-{}-memory-concurrency.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        r"
[memory]
concurrency = 8
",
    )
    .unwrap();

    assert!(ConfigLoader::load_from_path(&path).is_err());

    let _ = std::fs::remove_file(path);
}

#[test]
fn remote_mode_derives_internal_limits_and_validates_values() {
    let mut config = AxonConfig::default();

    assert_eq!(config.remote.mode, RemoteMode::Standard);
    assert_eq!(
        config.remote.max_events_per_cursor(),
        STANDARD_REMOTE_MAX_EVENTS_PER_CURSOR
    );
    assert_eq!(
        config.remote.max_payload_bytes(),
        STANDARD_REMOTE_MAX_PAYLOAD_BYTES
    );

    config.remote.mode = RemoteMode::HighThroughput;
    assert!(config.validate().is_ok());
    assert_eq!(
        config.remote.max_events_per_cursor(),
        HIGH_THROUGHPUT_REMOTE_MAX_EVENTS_PER_CURSOR
    );
    assert_eq!(
        config.remote.max_payload_bytes(),
        HIGH_THROUGHPUT_REMOTE_MAX_PAYLOAD_BYTES
    );
}

fn assert_target_config_keys_removed(specs: &[super::ConfigKeySpec]) {
    assert!(specs.iter().all(|spec| spec.key.starts_with("agent.")
        || spec.key.starts_with("remote.")
        || spec.key.starts_with("tui.")
        || spec.key.starts_with("memory.")
        || spec.key.starts_with("channel_balancer.")));
}
