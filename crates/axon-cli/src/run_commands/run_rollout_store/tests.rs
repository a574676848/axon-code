//! `run_rollout_store` SQLite 结构化回放测试。
//!
//! 本模块只保留共享夹具与子模块声明，用例按主题分散在同目录的兄弟文件里：
//! turn 读写、resume 链回放、会话家族保留、路径与窗口各自独立成文件。

#[path = "chain_tests.rs"]
mod chain_tests;
#[path = "path_tests.rs"]
mod path_tests;
#[path = "retention_tests.rs"]
mod retention_tests;
#[path = "turn_tests.rs"]
mod turn_tests;
#[path = "window_tests.rs"]
mod window_tests;

use super::{CLI_ROLLOUT_DB_FILE, RolloutSqliteStore, RunSessionRecord, resolved_rollout_db_path};
use axon_agent_runtime::agent_run_assistant_part_id;
use axon_session::{AXON_RUNTIME_DATA_DIR, SESSION_STORE_DIR, ThreadCompaction};

const FIXTURE_ASSISTANT_TURN_ID: u64 = 1;

pub(super) fn temp_db(tag: &str) -> std::path::PathBuf {
    let mut dir = axon_test_support::test_temp_root();
    let unique = format!(
        "axon_rollout_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    dir.push(unique);
    dir.push("rollout.sqlite3");
    dir
}

fn temp_config(tag: &str) -> std::path::PathBuf {
    temp_db(tag)
        .parent()
        .expect("临时数据库应有父目录")
        .join("config.toml")
}

pub(super) fn record(session: &str, parent: Option<&str>, created_at_ms: u64) -> RunSessionRecord {
    RunSessionRecord {
        session_id: session.to_owned(),
        parent_session_id: parent.map(ToOwned::to_owned),
        created_at_ms,
        startup_timeline: crate::run_commands::run_startup_snapshot::test_startup_timeline(),
        cwd: Some("/work".to_owned()),
        provider: "anthropic".to_owned(),
        model: "model-a".to_owned(),
        model_name: Some("model-a".to_owned()),
        prompt: format!("prompt-{session}"),
        local_images: Vec::new(),
        local_files: Vec::new(),
        selected_skill_name: None,
        repo_context: None,
        session_context: None,
        durable_memory: None,
        response: format!("response-{session}"),
        assistant_part_id: agent_run_assistant_part_id(FIXTURE_ASSISTANT_TURN_ID),
        usage: None,
        tool_receipts: vec![],
        tool_calls: vec![],
        reasoning_messages: vec![],
        turn_events: vec![],
        failure: None,
        upstream_model: None,
        reasoning_effort: None,
        request_path: None,
        reflection_notice: None,
        turn_trace: None,
        speed_mode: None,
        ttft_ms: None,
        generation_speed_tps: None,
        injected_context_messages: Vec::new(),
    }
}

pub(super) fn cleanup(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

#[path = "latest_model_tests.rs"]
mod latest_model_tests;
