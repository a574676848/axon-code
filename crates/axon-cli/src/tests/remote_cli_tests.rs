use std::fs;

use axon_remote_runtime::{RemoteNotificationDispatchStatus, RemoteSubscriptionStoreRuntime};
use axon_remote_feishu::{
    RemoteFeishuGovernanceDecisionStatus, RemoteFeishuGovernanceStoreRuntime,
};
use serde_json::Value;

use super::remote_cli_test_support::{
    seed_feishu_outbox, seed_feishu_websocket_config, seed_outbox, seed_subscription, test_root,
};
use super::{AxonCli, run_parsed_cli};

#[test]
fn remote_status_reports_local_governance_without_network_or_raw_payload() {
    let root = test_root("status");
    seed_subscription(&root, "secret-client", false);
    seed_outbox(
        &root,
        "dispatch-1",
        RemoteNotificationDispatchStatus::Pending,
    );
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--format", "json", "remote", "status",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("remote status 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "remote_ready");
    assert!(body["remoteRuntimeDir"]
        .as_str()
        .unwrap_or_default()
        .ends_with("remote"));
    assert_eq!(body["activeSubscriptionRecords"], 1);
    assert_eq!(body["pendingOutboxRecords"], 1);
    assert_eq!(body["networkSendAllowed"], false);
    assert_eq!(body["rawPayloadForwardAllowed"], false);
    assert!(!result.stdout.contains("secret-client"));
    assert!(!result.stdout.contains("secret-connection"));
    assert!(!result.stdout.contains("secret-pairing"));
    assert!(!result.stdout.contains("secret raw payload"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_subscriptions_lists_active_hash_projection_only() {
    let root = test_root("subscriptions");
    seed_subscription(&root, "secret-client", false);
    seed_subscription(&root, "secret-revoked", true);
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "remote",
        "subscriptions",
        "--limit",
        "10",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value =
        serde_json::from_str(&result.stdout).expect("remote subscriptions 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["activeRecords"], 1);
    assert_eq!(body["subscriptions"][0]["transport"], "webSocket");
    assert_eq!(body["subscriptions"][0]["networkSendAllowed"], false);
    assert!(!result.stdout.contains("secret-client"));
    assert!(!result.stdout.contains("secret-revoked"));
    assert!(!result.stdout.contains("secret-connection"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_revoke_appends_revoked_subscription_without_leaking_ids() {
    let root = test_root("revoke");
    seed_subscription(&root, "secret-client", false);
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "remote",
        "revoke",
        "7",
        "secret-client",
        "--reason",
        "operator secret reason",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("remote revoke 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "remote_revoked");
    assert_eq!(body["revoked"], true);
    assert_eq!(body["subscription"]["revoked"], true);
    assert_eq!(body["networkSendAllowed"], false);
    assert_eq!(body["rawPayloadPersisted"], false);
    assert!(!result.stdout.contains("secret-client"));
    assert!(!result.stdout.contains("secret-connection"));
    assert!(!result.stdout.contains("secret-pairing"));
    assert!(!result.stdout.contains("operator secret reason"));

    let store_path = RemoteSubscriptionStoreRuntime::resolve_store_path(root.join("data").join("remote"));
    let store_lines = axon_jsonl::ShardedJsonlStore::new()
        .read_lines(store_path)
        .expect("远控订阅 store 应可读取");
    let store_text = store_lines
        .into_iter()
        .map(|line| String::from_utf8(line).expect("远控订阅 store UTF-8"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!store_text.contains("operator secret reason"));
    assert!(store_text.contains("remote_subscription:revoked:sha256:"));

    let list_cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "remote",
        "subscriptions",
    ]);
    let list_result = run_parsed_cli(&list_cli);
    let list_body: Value =
        serde_json::from_str(&list_result.stdout).expect("remote subscriptions 应输出 JSON");
    assert_eq!(list_body["activeRecords"], 0);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_audit_reports_hash_only_revocation_and_outbox_summary() {
    let root = test_root("audit");
    seed_subscription(&root, "secret-client", false);
    seed_subscription(&root, "secret-revoked", true);
    seed_outbox(
        &root,
        "dispatch-1",
        RemoteNotificationDispatchStatus::Pending,
    );
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--format", "json", "remote", "audit", "--limit", "10",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("remote audit 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "remote_audit_ready");
    assert_eq!(body["latestSubscriptionRecords"], 2);
    assert_eq!(body["activeSubscriptionRecords"], 1);
    assert_eq!(body["revokedSubscriptionRecords"], 1);
    assert_eq!(body["pendingOutboxRecords"], 1);
    assert_eq!(body["subscriptions"][0]["networkSendAllowed"], false);
    assert_eq!(body["dispatches"][0]["rawPayloadForwardAllowed"], false);
    assert_eq!(body["networkSendAllowed"], false);
    assert!(!result.stdout.contains("secret-client"));
    assert!(!result.stdout.contains("secret-revoked"));
    assert!(!result.stdout.contains("secret-connection"));
    assert!(!result.stdout.contains("secret-pairing"));
    assert!(!result.stdout.contains("secret raw payload"));
    assert!(!result.stdout.contains("通知正文"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_outbox_lists_latest_dispatch_without_notification_body() {
    let root = test_root("outbox");
    seed_outbox(
        &root,
        "dispatch-1",
        RemoteNotificationDispatchStatus::Pending,
    );
    seed_outbox(
        &root,
        "dispatch-2",
        RemoteNotificationDispatchStatus::Delivered,
    );
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--format", "json", "remote", "outbox", "--limit", "10",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("remote outbox 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["totalRecords"], 2);
    assert_eq!(body["pendingRecords"], 1);
    assert_eq!(body["dispatches"][0]["localOnly"], true);
    assert_eq!(body["dispatches"][0]["rawPayloadPersisted"], false);
    assert!(!result.stdout.contains("secret-client"));
    assert!(!result.stdout.contains("secret raw payload"));
    assert!(!result.stdout.contains("通知正文"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_list_rejects_invalid_limit() {
    let root = test_root("limit");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "remote", "outbox", "--limit", "0",
    ]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 2);
    assert!(result.stderr.contains("remote limit"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_render_feishu_uses_registry_and_emits_bounded_payload() {
    let root = test_root("render-feishu");
    seed_feishu_outbox(&root, "dispatch-feishu");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    seed_feishu_websocket_config(&config_path);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "remote",
        "render-feishu",
        "9",
        "--limit",
        "5",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value =
        serde_json::from_str(&result.stdout).expect("remote render-feishu 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "remote_render_ready");
    assert_eq!(body["adapter"], "feishuWebSocket");
    assert_eq!(body["transport"], "feishu");
    assert_eq!(body["networkSendAllowed"], false);
    assert_eq!(body["rawPayloadForwardAllowed"], false);
    assert!(
        body["payload"]
            .as_str()
            .unwrap_or_default()
            .contains("\"template\":\"orange\"")
    );
    assert!(
        !body["payload"]
            .as_str()
            .unwrap_or_default()
            .contains("secret raw payload")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_feishu_status_reports_config_and_default_url() {
    let root = test_root("feishu-status");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    seed_feishu_websocket_config(&config_path);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "remote",
        "feishu-status",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value =
        serde_json::from_str(&result.stdout).expect("remote feishu-status 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "remote_feishu_websocket_ready");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["appIdConfigured"], true);
    assert_eq!(body["appSecretEnvConfigured"], true);
    assert_eq!(body["allowedSenderCount"], 1);
    assert_eq!(body["requireMention"], true);
    assert_eq!(body["botIdentityHydrated"], false);
    assert_eq!(body["usingDefaultWebsocketUrl"], true);
    assert_eq!(body["networkSendAllowed"], false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_simulate_feishu_message_runs_session_runtime_closed_loop() {
    let root = test_root("feishu-sim-message");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    seed_feishu_websocket_config(&config_path);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "remote",
        "simulate-feishu",
        "18",
        "message",
        "请汇总当前线程状态",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value =
        serde_json::from_str(&result.stdout).expect("remote simulate-feishu message 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "remote_feishu_simulation_ready");
    assert_eq!(body["mode"], "message");
    assert_eq!(body["governanceStatus"], "accepted");
    assert_eq!(body["payloadKind"], "messagePlan");
    assert_eq!(body["responseOnly"], true);
    assert_eq!(body["networkSendAllowed"], false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_simulate_feishu_message_rejects_sender_outside_allowlist() {
    let root = test_root("feishu-sim-allowlist");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    seed_feishu_websocket_config(&config_path);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "remote",
        "simulate-feishu",
        "18",
        "message",
        "未授权用户消息",
        "--sender-open-id",
        "ou_blocked",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value =
        serde_json::from_str(&result.stdout).expect("remote simulate-feishu allowlist 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["governanceStatus"], "rejectedByAllowlist");
    let governance = RemoteFeishuGovernanceStoreRuntime::read_records(
        RemoteFeishuGovernanceStoreRuntime::resolve_store_path(
            root.join("data").join("remote").join("feishu"),
        ),
    )
    .expect("simulate 后应写入治理记录");
    assert_eq!(governance.len(), 1);
    assert_eq!(
        governance[0].decision_status,
        RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_simulate_feishu_card_action_runs_session_runtime_closed_loop() {
    let root = test_root("feishu-sim-card");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    seed_feishu_websocket_config(&config_path);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "remote",
        "simulate-feishu",
        "19",
        "card-action",
        "button",
        "--action-json",
        "{\"axon_action\":\"clarify\"}",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout)
        .expect("remote simulate-feishu card-action 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "remote_feishu_simulation_ready");
    assert_eq!(body["mode"], "card-action");
    assert_eq!(body["payloadKind"], "messagePlan");
    assert_eq!(body["responseOnly"], true);
    let _ = fs::remove_dir_all(root);
}
