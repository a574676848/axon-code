use std::time::{SystemTime, UNIX_EPOCH};

use axon_config::provider_credential;

use super::{AxonCli, run_parsed_cli};

#[test]
fn debug_builtin_list_exposes_registered_builtin_tools() {
    let result = run_debug(["axon", "debug", "builtin", "list"]);

    assert_eq!(result.exit_code, 0);
    assert!(result.stderr.is_empty());
    assert!(result.stdout.contains("builtin.search"));
    assert!(result.stdout.contains("builtin.glob"));
    assert!(result.stdout.contains("builtin.shell_command"));
}

#[test]
fn debug_builtin_run_loads_config_adjacent_env_credentials() {
    let root = temp_dir("debug-env");
    let config_path = root.join("config.toml");
    axon_config::ConfigLoader::save_to_path(&config_path, &axon_config::AxonConfig::default())
        .unwrap();
    let env_name = format!(
        "AXON_DEBUG_BUILTIN_KEY_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::fs::write(
        root.join(".env"),
        format!("{env_name}=debug-builtin-secret\n"),
    )
    .unwrap();
    std::fs::write(root.join("marker.txt"), "marker\n").unwrap();
    let result = run_debug([
        "axon",
        "--config",
        config_path.to_string_lossy().as_ref(),
        "--cwd",
        root.to_string_lossy().as_ref(),
        "debug",
        "builtin",
        "run",
        "builtin.read_file",
        "--args",
        r#"{"path":"marker.txt"}"#,
    ]);

    assert_eq!(result.exit_code, 0, "{}", result.stdout);
    assert_eq!(
        provider_credential(&env_name).as_deref(),
        Some("debug-builtin-secret")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn debug_builtin_run_executes_readonly_tool_without_provider_loop() {
    let root = temp_dir("debug-search");
    std::fs::write(root.join("marker.txt"), "axon debug builtin marker\n").unwrap();
    let cwd_arg = root.to_string_lossy().into_owned();
    let result = run_debug([
        "axon",
        "--cwd",
        cwd_arg.as_str(),
        "debug",
        "builtin",
        "run",
        "builtin.search",
        "--args",
        r#"{"query":"marker","intent":"检索标记文本"}"#,
    ]);
    let _ = std::fs::remove_dir_all(root);

    assert_eq!(result.exit_code, 0, "{}", result.stdout);
    assert!(result.stdout.contains("Axon debug builtin builtin.search"));
    assert!(result.stdout.contains("marker.txt"));
}

fn run_debug<const N: usize>(args: [&str; N]) -> super::CliRunResult {
    let cli = AxonCli::parse_from_args(args);
    run_parsed_cli(&cli)
}

fn temp_dir(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = axon_test_support::test_temp_root().join(format!("axon-{label}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
