use super::{AxonCli, DoctorReport, DoctorStatus, run_parsed_cli};

#[test]
fn doctor_reports_release_and_runtime_health_by_default() {
    let cli = AxonCli::parse_from_args(["axon", "doctor"]);
    let result = run_parsed_cli(&cli);
    let json_cli =
        AxonCli::parse_from_args(["axon", "--format", "json", "doctor", "--check", "config"]);
    let json_result = run_parsed_cli(&json_cli);
    let value: serde_json::Value =
        serde_json::from_str(json_result.stdout.trim()).expect("doctor JSON 应可解析");

    assert!(
        result.exit_code == 0 || result.stdout.contains("smoke: failed"),
        "{}",
        result.stdout
    );
    assert!(result.stderr.is_empty());
    assert!(
        result.stdout.contains("Axon doctor: ok") || result.stdout.contains("Axon doctor: failed")
    );
    for check in ["config", "cli", "provider", "tool", "mcp", "memory", "lsp"] {
        assert!(result.stdout.contains(&format!("{check}: ok")));
    }
    assert!(
        result.stdout.contains("smoke: ok") || result.stdout.contains("smoke: failed"),
        "{}",
        result.stdout
    );
    assert_eq!(json_result.exit_code, 0);
    assert_eq!(value["status"], "ok");
    assert_eq!(value["checks"][0]["name"], "config");
}

#[test]
fn doctor_provider_recovery_receipt_passes_structured_validation() {
    let receipt_path = unique_doctor_receipt_path("provider-recovery");
    std::fs::write(
        &receipt_path,
        r#"{
            "schemaVersion": 1,
            "check": "provider-recovery",
            "recordedAtMs": 1000,
            "operator": "local-reviewer",
            "providerName": "external-openai-compatible",
            "protocol": "openai",
            "modelId": "real-model",
            "liveProvider": true,
            "simulatedProvider": false,
            "streamInterrupted": true,
            "resumeRetryObserved": true,
            "resumeMarkerObserved": true,
            "recoveredDeltaCount": 2,
            "recoveredContentChars": 18,
            "providerRawPayloadAllowed": false,
            "finalStatus": "ok"
        }"#,
    )
    .expect("测试回执应可写入");

    let receipt_arg = receipt_path.to_string_lossy().into_owned();
    let cli = AxonCli::parse_from_args([
        "axon",
        "doctor",
        "--check",
        "provider-recovery",
        "--provider-receipt",
        receipt_arg.as_str(),
    ]);
    let result = run_parsed_cli(&cli);
    let _ = std::fs::remove_file(&receipt_path);

    assert_eq!(result.exit_code, 0);
    assert!(result.stderr.is_empty());
    assert!(result.stdout.contains("provider-recovery: ok"));
    assert!(result.stdout.contains("外部真实 provider 恢复回执通过"));
}

#[test]
fn doctor_provider_recovery_receipt_fails_closed_without_receipt() {
    let cli = AxonCli::parse_from_args(["axon", "doctor", "--check", "provider-recovery"]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 1);
    assert!(result.stderr.is_empty());
    assert!(result.stdout.contains("需要 --provider-receipt"));
}

#[test]
fn doctor_provider_recovery_receipt_rejects_simulated_provider() {
    let receipt_path = unique_doctor_receipt_path("provider-recovery-simulated");
    std::fs::write(
        &receipt_path,
        r#"{
            "schemaVersion": 1,
            "check": "provider-recovery",
            "recordedAtMs": 1000,
            "operator": "local-reviewer",
            "providerName": "fixture",
            "protocol": "openai",
            "modelId": "fixture-model",
            "liveProvider": false,
            "simulatedProvider": true,
            "streamInterrupted": true,
            "resumeRetryObserved": true,
            "resumeMarkerObserved": true,
            "recoveredDeltaCount": 1,
            "recoveredContentChars": 4,
            "providerRawPayloadAllowed": false,
            "finalStatus": "ok"
        }"#,
    )
    .expect("测试回执应可写入");

    let receipt_arg = receipt_path.to_string_lossy().into_owned();
    let cli = AxonCli::parse_from_args([
        "axon",
        "doctor",
        "--check",
        "provider-recovery",
        "--provider-receipt",
        receipt_arg.as_str(),
    ]);
    let result = run_parsed_cli(&cli);
    let _ = std::fs::remove_file(&receipt_path);

    assert_eq!(result.exit_code, 1);
    assert!(result.stdout.contains("必须来自外部真实 provider"));
}

#[test]
fn doctor_provider_check_exposes_provider_strategy_summary() {
    let cli = AxonCli::parse_from_args(["axon", "doctor", "--check", "provider"]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 0);
    assert!(result.stderr.is_empty());
    assert!(result.stdout.contains("模型渠道已加载"));
    assert!(result.stdout.contains("channels="));
    assert!(result.stdout.contains("models="));
    assert!(result.stdout.contains("recommended_models="));
    assert!(result.stdout.contains("byok_channels="));
    assert!(result.stdout.contains("openai_compatible_channels="));
}

#[test]
fn doctor_tool_check_reports_turn_time_registry_summary() {
    let cli = AxonCli::parse_from_args(["axon", "doctor", "--check", "tool"]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 0, "{}", result.stdout);
    assert!(result.stderr.is_empty());
    assert!(
        result
            .stdout
            .contains("turn-time tool registry 已接入 shell/search/edit")
    );
    assert!(result.stdout.contains("registered_tools="));
}

#[test]
fn doctor_mcp_check_reports_turn_time_registry_summary() {
    let cli = AxonCli::parse_from_args(["axon", "doctor", "--check", "mcp"]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 0);
    assert!(result.stderr.is_empty());
    assert!(
        result
            .stdout
            .contains("MCP 管理命令与 turn-time registry 就绪")
    );
    assert!(result.stdout.contains("mcp_tools="));
}

#[test]
fn doctor_writes_provider_recovery_receipt_template_for_external_validation() {
    let receipt_path = unique_doctor_receipt_path("provider-template");
    let receipt_arg = receipt_path.to_string_lossy().into_owned();
    let cli = AxonCli::parse_from_args([
        "axon",
        "doctor",
        "--check",
        "provider-recovery",
        "--receipt-template",
        receipt_arg.as_str(),
    ]);
    let result = run_parsed_cli(&cli);
    let template = std::fs::read_to_string(&receipt_path).expect("模板应写入");
    let _ = std::fs::remove_file(&receipt_path);

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("receipt-template: ok"));
    assert!(template.contains(r#""check": "provider-recovery""#));
    assert!(template.contains(r#""simulatedProvider": false"#));
    assert!(template.contains(r#""providerRawPayloadAllowed": false"#));
}

#[test]
fn doctor_receipt_template_requires_template_capable_check() {
    let receipt_path = unique_doctor_receipt_path("unsupported-template");
    let receipt_arg = receipt_path.to_string_lossy().into_owned();
    let cli = AxonCli::parse_from_args([
        "axon",
        "doctor",
        "--check",
        "config",
        "--receipt-template",
        receipt_arg.as_str(),
    ]);
    let result = run_parsed_cli(&cli);
    let exists = receipt_path.exists();
    let _ = std::fs::remove_file(&receipt_path);

    assert_eq!(result.exit_code, 1);
    assert!(!exists);
    assert!(result.stdout.contains("没有回执模板"));
}

#[test]
fn doctor_unknown_check_fails_closed() {
    let cli = AxonCli::parse_from_args(["axon", "doctor", "--check", "missing"]);
    let result = run_parsed_cli(&cli);
    let report = DoctorReport::run(Some("missing"), false, false);

    assert_eq!(result.exit_code, 1);
    assert_eq!(report.status, DoctorStatus::Failed);
    assert!(result.stdout.contains("未知 doctor 检查项"));
}

#[test]
fn doctor_runtime_check_reports_writable_runtime_directory() {
    let cli = AxonCli::parse_from_args(["axon", "doctor", "--check", "runtime"]);
    let result = run_parsed_cli(&cli);
    assert!(result.stdout.contains("runtime: ok") || result.stdout.contains("runtime: failed"));
    assert!(result.stderr.is_empty());
}

fn unique_doctor_receipt_path(prefix: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    axon_test_support::test_temp_root().join(format!("axon-doctor-{prefix}-receipt-{unique}.json"))
}
