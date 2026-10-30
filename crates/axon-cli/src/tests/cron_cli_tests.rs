use std::fs;

use serde_json::Value;

use super::remote_cli_test_support::test_root;
use super::{AxonCli, run_parsed_cli};

fn stdout_script(text: &str) -> String {
    if cfg!(windows) {
        format!("Write-Output '{text}'")
    } else {
        format!("printf '%s\\n' '{text}'")
    }
}

#[test]
fn cron_create_projects_wake_target_and_uses_cron_store() {
    let root = test_root("cron-target-state");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    let create = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "cron",
        "create",
        "--name",
        "日报",
        "--schedule",
        "interval:1h",
        "--prompt",
        "总结今日进展",
        "--wake",
        "feishu:oc_cron_chat",
    ]);

    let create_result = run_parsed_cli(&create);
    let create_body: Value =
        serde_json::from_str(&create_result.stdout).expect("cron create 应输出 JSON");
    let job_id = create_body["jobs"][0]["jobId"]
        .as_str()
        .expect("cron create 应返回 jobId")
        .to_owned();
    let pause = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--format", "json", "cron", "pause", &job_id,
    ]);
    let pause_result = run_parsed_cli(&pause);
    let pause_body: Value =
        serde_json::from_str(&pause_result.stdout).expect("cron pause 应输出 JSON");

    assert_eq!(create_result.exit_code, 0, "{}", create_result.stderr);
    assert_eq!(create_body["status"], "cron_created");
    assert_eq!(create_body["jobs"][0]["wake"], "feishu:oc_cron_chat");
    assert!(
        create_body["storePath"]
            .as_str()
            .expect("应返回 storePath")
            .replace('\\', "/")
            .ends_with("/cron/cron.sqlite3")
    );
    assert_eq!(pause_result.exit_code, 0);
    assert_eq!(pause_body["jobs"][0]["status"], "paused");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cron_without_wake_runs_silently_without_advancing_schedule() {
    let root = test_root("cron-manual-run");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    let create = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "cron",
        "create",
        "--name",
        "静默探针",
        "--schedule",
        "interval:1h",
        "--no-agent",
        "--script",
        &stdout_script(""),
    ]);
    let create_body: Value =
        serde_json::from_str(&run_parsed_cli(&create).stdout).expect("cron create 应输出 JSON");
    let job_id = create_body["jobs"][0]["jobId"]
        .as_str()
        .expect("cron create 应返回 jobId")
        .to_owned();
    let next_run_at_ms = create_body["jobs"][0]["nextRunAtMs"].clone();
    let run = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--format", "json", "cron", "run", &job_id,
    ]);
    let run_result = run_parsed_cli(&run);
    let run_body: Value = serde_json::from_str(&run_result.stdout).expect("cron run 应输出 JSON");
    let list = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--format", "json", "cron", "list",
    ]);
    let list_body: Value =
        serde_json::from_str(&run_parsed_cli(&list).stdout).expect("cron list 应输出 JSON");

    assert_eq!(run_result.exit_code, 0);
    assert_eq!(run_body["runs"][0]["stdoutEmpty"], true);
    assert_eq!(run_body["runs"][0]["deliveryEnqueued"], false);
    assert_eq!(list_body["jobs"][0]["wake"], Value::Null);
    assert_eq!(list_body["jobs"][0]["revision"], 2);
    assert!(list_body["jobs"][0]["lastRunAtMs"].as_u64().is_some());
    assert_eq!(list_body["jobs"][0]["nextRunAtMs"], next_run_at_ms);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn legacy_feishu_flags_fail_with_chinese_migration_hint() {
    let root = test_root("cron-legacy-args");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    let create = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "cron",
        "create",
        "--name",
        "旧参数",
        "--schedule",
        "interval:1h",
        "--prompt",
        "执行",
        "--feishu-chat-id",
        "oc_legacy",
    ]);

    let result = run_parsed_cli(&create);

    assert_ne!(result.exit_code, 0, "旧参数必须失败而不是静默忽略");
    assert!(
        result.stderr.contains("--wake feishu:<chat_id>"),
        "错误里必须给出迁移路径：{}",
        result.stderr
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn create_rejects_unstable_session_wake_lane() {
    let root = test_root("cron-unstable-lane");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    let create = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "cron",
        "create",
        "--name",
        "不稳定车道",
        "--schedule",
        "interval:1h",
        "--prompt",
        "执行",
        "--wake",
        "session:tui:terminal-1",
    ]);

    let result = run_parsed_cli(&create);

    assert_ne!(result.exit_code, 0);
    assert!(
        result.stderr.contains("desktop"),
        "错误里必须说明受支持的渠道：{}",
        result.stderr
    );
    let _ = fs::remove_dir_all(root);
}
