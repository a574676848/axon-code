//! rollout 归档工具输出读取的端到端测试。

use std::path::{Path, PathBuf};

use axon_agent_runtime::{ContentArchiveRuntime, agent_run_assistant_part_id};
use serde_json::Value;

use super::{RolloutSqliteStore, RunSessionRecord};
use crate::{AxonCli, run_parsed_cli};

const FIXTURE_ASSISTANT_TURN_ID: u64 = 1;
const FIXTURE_ARCHIVED_TOOL_OUTPUT: &str = "完整归档正文";
const FIXTURE_PROVIDER_CALL_ID: &str = "provider-call-root";
const FIXTURE_REPLAY_SESSION_ID: &str = "replay-output-session";
const FIXTURE_TAMPERED_TOOL_OUTPUT: &str = "被篡改的归档正文";

#[test]
fn session_replay_reads_archived_output_only_after_explicit_provider_call_id() {
    let config_path = temp_config("replay-tool-output-cli");
    let runtime_root = config_path.parent().expect("配置文件应有父目录");
    std::fs::create_dir_all(runtime_root).unwrap();
    std::fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").unwrap();
    let archive_root = crate::runtime_paths::runtime_axon_data_path_static(&config_path)
        .expect("测试 Axon data 目录应可解析");
    let archive = ContentArchiveRuntime::new(1);
    let output_ref = archive
        .archive_text(
            &archive_root,
            axon_agent_runtime::CONTENT_ARCHIVE_RESULT_STREAM,
            FIXTURE_ARCHIVED_TOOL_OUTPUT,
            "builtin/shell_command",
        )
        .unwrap()
        .output_ref
        .expect("应生成 OutputRef");
    let path = super::resolved_rollout_db_path(&config_path).unwrap();
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut session = record(FIXTURE_REPLAY_SESSION_ID, 100);
    session.tool_receipts = vec![archived_tool_receipt(output_ref.clone())];
    store.record_session(&session).unwrap();
    drop(store);

    let archive_path = archive_root.join(
        &output_ref
            .archive_ref
            .as_ref()
            .expect("OutputRef 应保存相对归档路径")
            .relative_path,
    );
    std::fs::write(&archive_path, FIXTURE_TAMPERED_TOOL_OUTPUT).unwrap();
    let (normal_exit_code, normal) = replay_json(&config_path, None);
    assert_eq!(normal_exit_code, 0);
    assert!(
        normal["toolOutput"].is_null(),
        "普通 replay 不应读取归档正文"
    );

    std::fs::write(&archive_path, FIXTURE_ARCHIVED_TOOL_OUTPUT).unwrap();
    let (explicit_exit_code, explicit) = replay_json(&config_path, Some(FIXTURE_PROVIDER_CALL_ID));
    assert_eq!(explicit_exit_code, 0);
    assert_eq!(
        explicit["toolOutput"]["content"], FIXTURE_ARCHIVED_TOOL_OUTPUT,
        "显式 provider call id 应读取经过 receipt 约束和完整性校验的正文"
    );
    cleanup(&path);
}

fn archived_tool_receipt(
    output_ref: axon_domain::OutputRef,
) -> crate::run_commands::run_tool_loop::RunToolReceipt {
    crate::run_commands::run_tool_loop::RunToolReceipt {
        tool_call_id: 1,
        part_id: "tool-part:replay-output".to_owned(),
        provider_call_id: FIXTURE_PROVIDER_CALL_ID.to_owned(),
        tool_name: "builtin.shell_command".to_owned(),
        status: "completed".to_owned(),
        summary: FIXTURE_ARCHIVED_TOOL_OUTPUT.to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: Some(output_ref),
        duration_ms: Some(1),
    }
}

fn replay_json(config_path: &Path, provider_call_id: Option<&str>) -> (u8, Value) {
    let path_arg = config_path.to_string_lossy().to_string();
    let mut arguments = vec![
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "debug",
        "session",
        "replay",
        FIXTURE_REPLAY_SESSION_ID,
    ];
    if let Some(provider_call_id) = provider_call_id {
        arguments.extend(["--provider-call-id", provider_call_id]);
    }
    let result = run_parsed_cli(&AxonCli::parse_from_args(arguments));
    let body = serde_json::from_str(&result.stdout).expect("session replay 应输出 JSON");
    (result.exit_code, body)
}

fn record(session_id: &str, created_at_ms: u64) -> RunSessionRecord {
    RunSessionRecord {
        session_id: session_id.to_owned(),
        parent_session_id: None,
        created_at_ms,
        startup_timeline: crate::run_commands::run_startup_snapshot::test_startup_timeline(),
        cwd: Some("/work".to_owned()),
        provider: "anthropic".to_owned(),
        model: "model-a".to_owned(),
        selected_skill_name: None,
        repo_context: None,
        session_context: None,
        durable_memory: None,
        prompt: format!("prompt-{session_id}"),
        local_images: Vec::new(),
        local_files: Vec::new(),
        response: format!("response-{session_id}"),
        assistant_part_id: agent_run_assistant_part_id(FIXTURE_ASSISTANT_TURN_ID),
        usage: None,
        tool_receipts: Vec::new(),
        tool_calls: Vec::new(),
        reasoning_messages: Vec::new(),
        turn_events: Vec::new(),
        failure: None,
        upstream_model: None,
        reasoning_effort: None,
        request_path: None,
        speed_mode: None,
        ttft_ms: None,
        generation_speed_tps: None,
        injected_context_messages: Vec::new(),
    }
}

fn temp_config(tag: &str) -> PathBuf {
    let unique = format!(
        "axon_rollout_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    axon_test_support::test_temp_root()
        .join(unique)
        .join("config.toml")
}

fn cleanup(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}
