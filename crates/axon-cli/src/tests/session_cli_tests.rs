use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_session::{JsonlThreadEventStore, ThreadEventRecord, ThreadEventStore};
use serde_json::Value;

use super::{AxonCli, run_parsed_cli};
use crate::run_commands::{
    RolloutEventAppendInput, RolloutEventRecord, append_rollout_event_for_session,
};

#[test]
fn session_replay_reads_rollout_events_without_run_session_record() {
    let root = test_root("replay-rollout-only");
    fs::create_dir_all(&root).expect("测试目录应可创建");
    let config_path = root.join("config.toml");
    fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").expect("测试配置应可写入");
    let rollout_events = [
        r#"{"schemaVersion":1,"sessionId":"cli-session-1","parentSessionId":null,"sequence":0,"eventType":"turn_events","recordedAtMs":100,"payload":{"source":"run_prompt","runId":"cli-run-1","turnEvents":[{"turnId":1,"kind":"TurnStarted","recordedAtMs":100,"summary":"provider turn 1 started"},{"turnId":1,"kind":"Completed","recordedAtMs":101,"summary":"provider turn finished without additional tool calls"}]}}"#,
    ];
    seed_rollout_events(&config_path, &rollout_events);

    let path_arg = config_path.to_string_lossy().to_string();
    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "debug",
        "session",
        "replay",
        "cli-session-1",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("session replay 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "session_replayed");
    assert_eq!(body["sessionId"], "cli-session-1");
    assert_eq!(body["chainLen"], 0);
    assert_eq!(body["eventCount"], 1);
    assert_eq!(body["eventSummaries"][0]["source"], "run_prompt");
    assert_eq!(body["eventSummaries"][0]["turnEventCount"], 2);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn thread_events_remain_readable() {
    let root = test_root("thread-readable");
    seed_sessions(&root);
    let path_arg = root.join("config.toml").to_string_lossy().to_string();

    let doctor = run_json_cli([
        "axon", "--config", &path_arg, "--format", "json", "debug", "session", "doctor",
    ]);
    assert_eq!(doctor.0.exit_code, 0);
    assert_eq!(doctor.1["status"], "session_doctor_ok");

    let list = run_json_cli([
        "axon", "--config", &path_arg, "--format", "json", "debug", "session", "list",
    ]);
    assert_eq!(list.0.exit_code, 0);
    assert!(
        list.1["sessions"]
            .as_array()
            .is_some_and(|sessions| !sessions.is_empty())
    );

    let show = run_json_cli([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "debug",
        "session",
        "show",
        "session-old",
    ]);
    assert_eq!(show.0.exit_code, 0);
    assert_eq!(show.1["session"]["threadId"], "session-old");

    let read = run_json_cli([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "debug",
        "session",
        "read",
        "session-old",
    ]);
    assert_eq!(read.0.exit_code, 0);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_list_isolates_unreadable_shards_and_doctor_reports_them() {
    let root = test_root("unreadable-shard");
    seed_sessions(&root);
    let event_directory = JsonlThreadEventStore::path(&root);
    fs::write(event_directory.join("broken.jsonl"), "{}\n").expect("损坏 shard fixture 应可写入");
    let path_arg = root.join("config.toml").to_string_lossy().to_string();

    let list = run_json_cli([
        "axon", "--config", &path_arg, "--format", "json", "debug", "session", "list",
    ]);
    assert_eq!(list.0.exit_code, 0);
    assert_eq!(list.1["status"], "session_ready");
    assert_eq!(list.1["unreadableThreadShards"], 1);
    assert!(
        list.1["warnings"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(
        list.1["sessions"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    let doctor = run_json_cli([
        "axon", "--config", &path_arg, "--format", "json", "debug", "session", "doctor",
    ]);
    assert_eq!(doctor.0.exit_code, 0);
    assert_eq!(doctor.1["status"], "session_doctor_failed");
    assert_eq!(doctor.1["unreadableThreadShards"], 1);
    assert_eq!(doctor.1["checks"][0]["name"], "event_store");
    assert_eq!(doctor.1["checks"][0]["status"], "failed");
    assert_eq!(doctor.1["checks"][1]["name"], "projection");
    assert_eq!(doctor.1["checks"][1]["status"], "ok");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_read_text_outputs_thread_and_turn_details() {
    let root = test_root("read-text-details");
    seed_sessions(&root);
    let path_arg = root.join("config.toml").to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "debug",
        "session",
        "read",
        "session-old",
    ]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("Thread: session-old"));
    assert!(result.stdout.contains("- provider: local"));
    assert!(result.stdout.contains("- model: small"));
    assert!(result.stdout.contains("Turn 0 · Completed"));
    assert!(result.stdout.contains("- user: 旧敏感提示"));
    assert!(result.stdout.contains("- assistant: 第二轮回答"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_replay_summarizes_tool_and_memory_receipt_fields() {
    let root = test_root("replay-rollout-summary");
    fs::create_dir_all(&root).expect("测试目录应可创建");
    let config_path = root.join("config.toml");
    fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").expect("测试配置应可写入");
    let rollout_events = [
        r#"{"schemaVersion":1,"sessionId":"agent-pane-provider-session-1000001","parentSessionId":null,"sequence":0,"eventType":"turn_events","recordedAtMs":100,"payload":{"source":"optional_ui_agent_pane","paneId":1,"cellId":1,"turnEvents":[{"turnId":1,"kind":"TurnStarted","recordedAtMs":100,"summary":"provider turn 1 started"}],"toolReceipts":[{"toolCallId":1000001,"toolName":"builtin_search","status":"Completed"}]}}"#,
        r#"{"schemaVersion":1,"sessionId":"agent-pane-provider-session-1000001","parentSessionId":null,"sequence":1,"eventType":"session_completed","recordedAtMs":101,"payload":{"source":"optional_ui_memory_receipt","hook":"MemoryUpdate","status":"CompletedNoWrite","writeDecision":"SkippedNoChange","reason":"auto candidate skipped","memoryUpdatedInfoVisible":false}}"#,
    ];
    seed_rollout_events(&config_path, &rollout_events);

    let path_arg = config_path.to_string_lossy().to_string();
    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "debug",
        "session",
        "replay",
        "agent-pane-provider-session-1000001",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("session replay 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["eventCount"], 2);
    assert_eq!(body["eventSummaries"][0]["toolReceiptCount"], 1);
    assert_eq!(
        body["eventSummaries"][1]["memoryWriteStatus"],
        "SkippedNoChange"
    );

    let _ = fs::remove_dir_all(root);
}

fn run_json_cli<const N: usize>(args: [&str; N]) -> (super::CliRunResult, Value) {
    let cli = AxonCli::parse_from_args(args);
    let result = run_parsed_cli(&cli);
    let body = serde_json::from_str(&result.stdout).expect("命令应输出 JSON");
    (result, body)
}

fn seed_rollout_events(config_path: &Path, events: &[&str]) {
    for event in events {
        let record: RolloutEventRecord =
            serde_json::from_str(event).expect("rollout event fixture 应有效");
        append_rollout_event_for_session(
            config_path,
            RolloutEventAppendInput {
                session_id: record.session_id,
                parent_session_id: record.parent_session_id,
                event_type: record.event_type,
                recorded_at_ms: record.recorded_at_ms,
                payload: record.payload,
            },
        )
        .expect("rollout event 应写入 SQLite");
    }
}

fn seed_sessions(root: &Path) {
    fs::create_dir_all(root).expect("测试根目录应可创建");
    fs::write(
        root.join("config.toml"),
        "[agent]\ndefault_model = \"balanced\"\n",
    )
    .expect("测试配置应可写入");
    let content = [
        thread_started("session-old", None, 0, 1000, "small"),
        turn_completed("session-old", 1, 0, 1001, "旧敏感提示", "旧敏感回答"),
        turn_completed("session-old", 2, 1, 1002, "第二轮提示", "第二轮回答"),
        thread_started("session-new", Some("session-old"), 3, 2000, "large"),
        turn_completed("session-new", 4, 0, 2001, "敏感提示AB", "敏感回答"),
    ];
    let store = JsonlThreadEventStore;
    for value in content {
        let record =
            serde_json::from_str::<ThreadEventRecord>(&value).expect("测试 thread 事件应有效");
        store
            .append(root, &record)
            .expect("测试 thread 事件应可写入");
    }
}

fn thread_started(
    thread_id: &str,
    parent_thread_id: Option<&str>,
    sequence: u64,
    recorded_at_ms: u64,
    model: &str,
) -> String {
    serde_json::json!({
        "schemaVersion": 3,
        "threadId": thread_id,
        "sequence": sequence,
        "recordedAtMs": recorded_at_ms,
        "event": {
            "type": "threadStarted",
            "session_root_id": thread_id,
            "parent_thread_id": parent_thread_id,
            "forked_from_thread_id": parent_thread_id,
            "source": "cliRun",
            "ephemeral": false,
            "settings": {
                "cwd": "E:\\fixture",
                "provider": "local",
                "model": model,
                "selectedSkillName": null
            }
        }
    })
    .to_string()
}

fn turn_completed(
    thread_id: &str,
    sequence: u64,
    turn_sequence: u64,
    recorded_at_ms: u64,
    user_message: &str,
    assistant_message: &str,
) -> String {
    serde_json::json!({
        "schemaVersion": 3,
        "threadId": thread_id,
        "sequence": sequence,
        "recordedAtMs": recorded_at_ms,
        "event": {
            "type": "turnCompleted",
            "turn": {
                "turnId": format!("{thread_id}-turn-{turn_sequence}"),
                "sequence": turn_sequence,
                "status": "completed",
                "historyItems": [
                    {
                        "type": "userMessage",
                        "sequence": 0,
                        "part_id": format!("{thread_id}-user-{turn_sequence}"),
                        "content": user_message,
                        "local_images": [],
                        "local_files": [],
                        "provider_metadata": null
                    },
                    {
                        "type": "assistantMessage",
                        "sequence": 1,
                        "part_id": format!("{thread_id}-assistant-{turn_sequence}"),
                        "content": assistant_message,
                        "provider_metadata": null
                    }
                ],
                "tokenUsageJson": null,
                "startedAtMs": recorded_at_ms,
                "completedAtMs": recorded_at_ms,
                "interruptedAtMs": null
            }
        }
    })
    .to_string()
}

fn test_root(name: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-session-cli-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}
