use std::fs;

use axon_config::AxonConfig;
use serde_json::Value;

use super::{AxonCli, run_parsed_cli, seed_memory_records, test_root, write_config};

#[test]
fn memory_list_filters_records_without_source_summary_raw_text() {
    let root = test_root("list");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_memory_records(&root);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "list",
        "--kind",
        "strong",
        "--limit",
        "4",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("memory list 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    let records = body["records"].as_array().expect("records 应为数组");
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|record| record["state"] == "strong"));
    let debug_record = records
        .iter()
        .find(|record| record["contentRef"] == "memory-ref:中文 检索 调试 经验")
        .expect("strong 列表应包含中文调试经验");
    assert_eq!(debug_record["memorySummary"], "Memory CLI 脱敏测试摘要");
    assert!(!result.stdout.contains("secret-source-summary"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_list_filters_by_stable_memory_kind() {
    let root = test_root("list-memory-kind");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_memory_records(&root);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "list",
        "--memory-kind",
        "knowledge",
        "--limit",
        "4",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("memory list 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["memoryKind"], "knowledge");
    let records = body["records"].as_array().expect("records 应为数组");
    assert_eq!(records.len(), 4);
    assert!(
        records
            .iter()
            .all(|record| record["memoryKind"] == "knowledge")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_search_uses_hybrid_retrieval_and_ref_projection() {
    let root = test_root("search");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_memory_records(&root);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "search",
        "中文 检索",
        "--limit",
        "4",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("memory search 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["hits"].as_array().expect("hits 应为数组").len(), 1);
    assert_eq!(
        body["hits"][0]["record"]["contentRef"],
        "memory-ref:中文 检索 调试 经验"
    );
    assert_eq!(body["hits"][0]["record"]["promptTextPersisted"], false);
    assert_eq!(body["hits"][0]["record"]["rawPayloadPersisted"], false);
    assert_eq!(body["hits"][0]["record"]["remoteWriteAllowed"], false);
    assert!(!result.stdout.contains("secret-source-summary"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_doctor_reports_drift_audit_without_raw_text() {
    let root = test_root("doctor");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_memory_records(&root);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "doctor",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("memory doctor 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "memory_doctor_ready");
    assert!(body["totalRecords"].as_u64().unwrap_or_default() >= 4);
    assert_eq!(body["repairRecommended"], false);
    assert_eq!(body["refOnly"], true);
    assert_eq!(body["remoteWriteAllowed"], false);
    assert!(!result.stdout.contains("secret-source-summary"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_review_lists_pending_records_without_raw_text() {
    let root = test_root("review");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_memory_records(&root);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "review",
        "--limit",
        "4",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("memory review 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "memory_review_ready");
    assert_eq!(body["remoteReviewAllowed"], false);
    assert_eq!(body["rawPayloadExported"], false);
    let records = body["records"].as_array().expect("records 应为数组");
    assert_eq!(records.len(), 3);
    let review_record = records
        .iter()
        .find(|record| record["record"]["contentRef"] == "memory-ref:review-quarantine-policy")
        .expect("review 列表应包含本机复核策略");
    assert_eq!(review_record["record"]["state"], "weak");
    assert_eq!(
        review_record["reviewReasons"][0],
        "weak_memory_requires_local_review"
    );
    assert!(
        review_record["recommendedActions"]
            .as_array()
            .expect("recommendedActions 应为数组")
            .iter()
            .any(|action| action == "quarantine")
    );
    assert!(!result.stdout.contains("secret-source-summary"));
    let _ = fs::remove_dir_all(root);
}
