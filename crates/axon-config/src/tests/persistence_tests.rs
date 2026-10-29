use super::*;
use crate::{DEFAULT_BASE_URL_ANTHROPIC, DEFAULT_BASE_URL_OPENAI, DEFAULT_MODEL_CONTEXT_WINDOW};

#[test]
fn user_config_file_save_and_load_roundtrip() {
    let path = axon_test_support::test_temp_root()
        .join(format!("axon-config-{}-roundtrip.toml", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut config = AxonConfig::default();
    config.remote.mode = RemoteMode::HighThroughput;
    config.providers.push(crate::ProviderConfig::new(
        "openai",
        "openai",
        "https://api.openai.com/v1",
    ));
    config.models.push(ModelConfig {
        name: "gpt-4o".to_owned(),
        id: "gpt-4o".to_owned(),
        provider: "openai".to_owned(),
        context_window: 128_000,
        ..ModelConfig::default()
    });
    config.agent.default_model = "gpt-4o".to_owned();
    config.agent.fast_model = "gpt-4o".to_owned();

    ConfigLoader::save_to_path(&path, &config).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(!saved.contains("[vision]"));
    assert!(!saved.contains("[goals]"));
    assert!(!saved.contains("[apply_edit]"));
    assert!(!saved.contains("[attachment]"));
    assert!(!saved.contains("dimension ="));
    assert!(!saved.contains("stdout_log_policy"));
    assert!(!saved.contains("model_catalog_sync_interval_secs"));
    assert!(!saved.contains("enable_failover"));
    assert!(!saved.contains("memory.enabled"));
    assert!(!saved.contains("memory_internal"));
    assert!(!saved.contains("concurrency ="));
    assert!(!saved.contains("reasoning_effort = \"medium\""));
    assert!(saved.contains("mode = \"high-throughput\""));
    let loaded = ConfigLoader::load_from_path(&path).unwrap();

    assert_eq!(loaded.config.remote.mode, RemoteMode::HighThroughput);

    let _ = std::fs::remove_file(path);
}

#[test]
fn default_user_config_path_uses_platform_specific_root() {
    let windows_root = default_user_axon_root_path_from_env(
        true,
        Some(std::ffi::OsStr::new("C:\\Users\\demo")),
        Some(std::ffi::OsStr::new("/home/demo")),
    )
    .unwrap();
    assert_eq!(
        windows_root.join(USER_CONFIG_FILE),
        std::path::PathBuf::from("C:\\Users\\demo")
            .join(USER_AXON_DIR)
            .join(USER_CONFIG_FILE)
    );

    let unix_root = default_user_axon_root_path_from_env(
        false,
        Some(std::ffi::OsStr::new("C:\\Users\\demo")),
        Some(std::ffi::OsStr::new("/home/demo")),
    )
    .unwrap();
    assert_eq!(
        unix_root.join(USER_CONFIG_FILE),
        std::path::PathBuf::from("/home/demo")
            .join(USER_AXON_DIR)
            .join(USER_CONFIG_FILE)
    );
}

#[test]
fn runtime_root_uses_user_root_for_default_and_project_configs() {
    let user_root = crate::default_user_axon_root_path_from_env(
        cfg!(windows),
        std::env::var_os("USERPROFILE").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
    .unwrap();
    let default_config = user_root.join(USER_CONFIG_FILE);
    let project_config = axon_test_support::test_temp_root()
        .join(format!("axon-config-project-{}", std::process::id()))
        .join(USER_AXON_DIR)
        .join(USER_CONFIG_FILE);

    assert_eq!(
        crate::runtime_axon_root_path(&default_config).unwrap(),
        user_root
    );
    assert_eq!(
        crate::runtime_axon_root_path(&project_config).unwrap(),
        crate::default_user_axon_root_path().unwrap()
    );
}

#[test]
fn runtime_root_uses_config_parent_for_explicit_custom_config() {
    let root = axon_test_support::test_temp_root()
        .join(format!("axon-config-runtime-root-{}", std::process::id()));
    let custom_config = root.join("isolated.toml");

    assert_eq!(crate::runtime_axon_root_path(&custom_config).unwrap(), root);
}

#[test]
fn retired_agent_model_slot_is_ignored() {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-config-{}-unknown-fields.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let retired_key = ["reasoning", "model"].join("_");
    std::fs::write(&path, format!("[agent]\n{retired_key} = \"retired\"\n")).unwrap();

    // 已移除的槽位属于未声明字段：加载忽略，保存时被丢弃。
    let loaded = ConfigLoader::load_from_path(&path).unwrap();
    let serialized = crate::to_toml_string(&loaded.config).unwrap();
    assert!(!serialized.contains(&retired_key));

    let _ = std::fs::remove_file(path);
}

#[test]
fn save_to_path_backs_up_only_before_content_changes() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-config-backup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");
    let original = AxonConfig::default();
    ConfigLoader::save_to_path(&path, &original).unwrap();

    ConfigLoader::save_to_path(&path, &original).unwrap();
    assert_eq!(config_backup_count(&root), 0);

    let mut updated = original;
    updated.remote.mode = crate::RemoteMode::HighThroughput;
    ConfigLoader::save_to_path(&path, &updated).unwrap();
    assert_eq!(config_backup_count(&root), 1);
    let backup = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("config.toml.backup-")
        })
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(backup.path()).unwrap(),
        crate::to_toml_string(&AxonConfig::default()).unwrap()
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("mode = \"high-throughput\"")
    );
}

#[test]
fn write_with_backup_overwrites_in_place_and_keeps_a_copy() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-config-write-with-backup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join(".env");
    std::fs::write(&path, "FIRST=1\n").unwrap();

    // 目标已存在：先落一份备份，再覆写原文件本身。
    let backup = crate::write_with_backup(&path, b"FIRST=1\nSECOND=2\n")
        .unwrap()
        .expect("已存在的文件必须留下备份");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "FIRST=1\nSECOND=2\n"
    );
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "FIRST=1\n");
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".env.backup-")
    );
    // 覆写而不是替换：文件身份（inode/句柄）保持不变，也不需要删除权限。
    let metadata_before = std::fs::metadata(&path).unwrap();
    crate::write_with_backup(&path, b"FIRST=9\n").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "FIRST=9\n");
    let metadata_after = std::fs::metadata(&path).unwrap();
    assert_eq!(
        metadata_before.created().ok(),
        metadata_after.created().ok(),
        "覆写必须保留原文件身份"
    );

    // 目标不存在：没有可备份的内容，直接创建。
    let fresh = root.join("fresh.env");
    assert!(
        crate::write_with_backup(&fresh, b"KEY=1\n")
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "KEY=1\n");
}

/// 直接写入不得在目标目录留下临时文件残留。
#[test]
fn save_to_path_writes_in_place_without_leaving_temporary_files() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-config-in-place-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");

    ConfigLoader::save_to_path(&path, &AxonConfig::default()).unwrap();

    let mut updated = AxonConfig::default();
    updated.remote.mode = crate::RemoteMode::HighThroughput;
    ConfigLoader::save_to_path(&path, &updated).unwrap();

    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("mode = \"high-throughput\"")
    );

    let leftovers: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "config.toml" && !name.starts_with("config.toml.backup-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "写入后目录内残留了非预期文件：{leftovers:?}"
    );
}

/// 直接写入必须保留目标既有权限位。
#[cfg(unix)]
#[test]
fn save_to_path_preserves_existing_file_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let root = axon_test_support::test_temp_root().join(format!(
        "axon-config-perm-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");

    ConfigLoader::save_to_path(&path, &AxonConfig::default()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let mut updated = AxonConfig::default();
    updated.remote.mode = crate::RemoteMode::HighThroughput;
    ConfigLoader::save_to_path(&path, &updated).unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "写入后权限位被改变");
}

#[cfg(windows)]
#[test]
fn save_to_path_supports_editor_handle_without_delete_sharing() {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;

    const EDITOR_SHARE_MODE: u32 = 0x0000_0001 | 0x0000_0002;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    let original = AxonConfig::default();
    ConfigLoader::save_to_path(&path, &original).unwrap();
    let mut editor = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(EDITOR_SHARE_MODE)
        .open(&path)
        .unwrap();
    let mut updated = original.clone();
    updated.agent.reasoning_effort = "high".to_owned();
    assert!(crate::atomic_write::replace_file_atomically(&path, b"blocked").is_err());
    ConfigLoader::save_to_path(&path, &updated).unwrap();
    editor.seek(SeekFrom::Start(0)).unwrap();
    let mut content = String::new();
    editor.read_to_string(&mut content).unwrap();
    assert_eq!(content, crate::to_toml_string(&updated).unwrap());
    assert_eq!(super::config_backup_count(root.path()), 1);
}

#[test]
fn default_base_url_and_context_window_are_pruned_on_save_and_restored_on_load() {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-config-{}-prune-test.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    let mut config = AxonConfig::default();
    config.providers.push(crate::ProviderConfig::new(
        "openai",
        "openai",
        DEFAULT_BASE_URL_OPENAI,
    ));
    config.providers.push(crate::ProviderConfig::new(
        "anthropic",
        "anthropic",
        DEFAULT_BASE_URL_ANTHROPIC,
    ));
    config.providers.push(crate::ProviderConfig::new(
        "custom",
        "openai",
        "https://custom.openai.com/v1",
    ));

    config.models.push(ModelConfig {
        name: "gpt-4o".to_owned(),
        id: "gpt-4o".to_owned(),
        provider: "openai".to_owned(),
        context_window: DEFAULT_MODEL_CONTEXT_WINDOW,
        ..ModelConfig::default()
    });
    config.models.push(ModelConfig {
        name: "text-embedding-3".to_owned(),
        id: "text-embedding-3".to_owned(),
        provider: "openai".to_owned(),
        supports_embedding: true,
        embedding_dimension: 1536,
        context_window: 0,
        ..ModelConfig::default()
    });
    config.agent.default_model = "gpt-4o".to_owned();
    config.agent.fast_model = "gpt-4o".to_owned();

    ConfigLoader::save_to_path(&path, &config).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();

    // 校验：默认 base_url 和默认 context_window 不落盘
    assert!(!saved.contains("base_url = \"https://api.openai.com/v1\""));
    assert!(!saved.contains("base_url = \"https://api.anthropic.com/v1\""));
    assert!(saved.contains("base_url = \"https://custom.openai.com/v1\""));
    assert!(!saved.contains("context_window = 128000"));

    // 校验：加载后能够补全恢复
    let loaded = ConfigLoader::load_from_path(&path).unwrap();
    let p_openai = loaded
        .config
        .providers
        .iter()
        .find(|p| p.name == "openai")
        .unwrap();
    assert_eq!(p_openai.base_url, DEFAULT_BASE_URL_OPENAI);
    let p_anthropic = loaded
        .config
        .providers
        .iter()
        .find(|p| p.name == "anthropic")
        .unwrap();
    assert_eq!(p_anthropic.base_url, DEFAULT_BASE_URL_ANTHROPIC);
    let p_custom = loaded
        .config
        .providers
        .iter()
        .find(|p| p.name == "custom")
        .unwrap();
    assert_eq!(p_custom.base_url, "https://custom.openai.com/v1");

    let m_gpt4o = loaded
        .config
        .models
        .iter()
        .find(|m| m.name == "gpt-4o")
        .unwrap();
    assert_eq!(m_gpt4o.context_window, DEFAULT_MODEL_CONTEXT_WINDOW);

    let m_emb = loaded
        .config
        .models
        .iter()
        .find(|m| m.name == "text-embedding-3")
        .unwrap();
    assert_eq!(m_emb.context_window, 0);

    let _ = std::fs::remove_file(path);
}
