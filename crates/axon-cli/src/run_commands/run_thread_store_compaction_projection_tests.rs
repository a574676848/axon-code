//! durable 压缩投影（`install_compaction_projection`）的 CAS 与回滚不变量测试。
//!
//! 环境隔离：fixture 的 config 落在 `axon_test_support::test_temp_dir()` 内。`runtime_axon_root_path` 只在
//! config 等于 `~/.axon/config.toml`、或其父目录名为 `.axon` 时回落到真实用户根目录；
//! 本 fixture 两者都不成立，因此 rollout SQLite 与 thread JSONL 全部写在临时目录，
//! 并由 `TempDir` 在用例结束时回收，绝不触碰用户运行时目录。该性质由
//! `CompactionStoreFixture::new` 内的断言锁定，改名或搬迁 fixture 时必须同步复核。

use super::*;
use axon_agent_runtime::agent_run_assistant_part_id;
use axon_session::{
    JsonlThreadEventStore, NameThreadCommand, StartThreadCommand, ThreadCompaction, ThreadEvent,
    ThreadEventStore as _, ThreadHistoryItem, ThreadService, ThreadSettings, ThreadSource, Turn,
    TurnStatus,
};

use crate::run_commands::{
    run_rollout_store::{
        commit_session_to_rollout, install_rollout_compaction, load_rollout_history_window,
    },
    run_session_store::RunSessionRecord,
};

const COMPACTION_TEST_SESSION_ID: &str = "compaction-cas-session";

#[test]
fn projection_uses_thread_event_sequence_instead_of_rollout_turn_sequence() {
    let fixture = CompactionStoreFixture::new();
    let expected_thread_sequence =
        thread_event_sequence_snapshot(fixture.root(), COMPACTION_TEST_SESSION_ID).unwrap();
    let compaction = fixture_compaction("最新摘要", 300);

    assert_ne!(expected_thread_sequence, compaction.through_sequence);
    install_compaction_projection(
        &fixture.config_path,
        fixture.root(),
        COMPACTION_TEST_SESSION_ID,
        expected_thread_sequence,
        compaction.clone(),
        None,
    )
    .unwrap();

    let projected = ThreadService::new(JsonlThreadEventStore)
        .read_thread(fixture.root(), COMPACTION_TEST_SESSION_ID)
        .unwrap();
    assert_eq!(projected.compacted_history, Some(compaction.clone()));
    assert_eq!(
        load_rollout_history_window(&fixture.config_path, COMPACTION_TEST_SESSION_ID)
            .unwrap()
            .compaction,
        Some(compaction)
    );
}

#[test]
fn projection_conflict_restores_previous_rollout_checkpoint() {
    let fixture = CompactionStoreFixture::new();
    let previous = fixture_compaction("旧摘要", 200);
    install_rollout_compaction(&fixture.config_path, COMPACTION_TEST_SESSION_ID, &previous)
        .unwrap();
    let expected_thread_sequence =
        thread_event_sequence_snapshot(fixture.root(), COMPACTION_TEST_SESSION_ID).unwrap();
    ThreadService::new(JsonlThreadEventStore)
        .name_thread(
            fixture.root(),
            NameThreadCommand {
                thread_id: COMPACTION_TEST_SESSION_ID.to_owned(),
                name: Some("摘要期间更新".to_owned()),
                recorded_at_ms: 250,
            },
        )
        .unwrap();

    let error = install_compaction_projection(
        &fixture.config_path,
        fixture.root(),
        COMPACTION_TEST_SESSION_ID,
        expected_thread_sequence,
        fixture_compaction("不应安装", 300),
        Some(&previous),
    )
    .unwrap_err();

    assert!(error.contains("version conflict"));
    assert!(error.contains("已回滚 rollout"));
    assert_eq!(
        load_rollout_history_window(&fixture.config_path, COMPACTION_TEST_SESSION_ID)
            .unwrap()
            .compaction,
        Some(previous)
    );
    let records = JsonlThreadEventStore
        .read_thread(fixture.root(), COMPACTION_TEST_SESSION_ID)
        .unwrap();
    assert!(
        records
            .iter()
            .all(|record| !matches!(record.event, ThreadEvent::ThreadCheckpoint { .. }))
    );
}

struct CompactionStoreFixture {
    config_path: std::path::PathBuf,
    // 字段顺序即析构顺序：临时目录必须最后析构，运行态句柄先释放，目录才删得掉。
    _temp: axon_test_support::TestTempDir,
}

impl CompactionStoreFixture {
    fn new() -> Self {
        let temp = axon_test_support::test_temp_dir();
        let config_path = temp.path().join("runtime-config.toml");
        // 真实环境隔离断言：解析出的运行态根目录必须是临时目录本身。
        assert_eq!(
            resolved_axon_root(&config_path).unwrap().as_path(),
            temp.path(),
            "fixture 的 config 必须让 runtime root 落在临时目录，否则会写进真实 Axon 用户目录"
        );
        let record = fixture_session_record();
        commit_session_to_rollout(&config_path, &record, "CAS 测试").unwrap();
        ThreadService::new(JsonlThreadEventStore)
            .start_thread(
                temp.path(),
                StartThreadCommand {
                    thread_id: COMPACTION_TEST_SESSION_ID.to_owned(),
                    session_root_id: COMPACTION_TEST_SESSION_ID.to_owned(),
                    parent_thread_id: None,
                    forked_from_thread_id: None,
                    source: ThreadSource::CliRun,
                    ephemeral: false,
                    settings: fixture_thread_settings(),
                    created_at_ms: 100,
                    turn: fixture_turn(),
                },
            )
            .unwrap();
        Self {
            config_path,
            _temp: temp,
        }
    }

    fn root(&self) -> &std::path::Path {
        self.config_path.parent().unwrap()
    }
}

fn fixture_session_record() -> RunSessionRecord {
    RunSessionRecord {
        session_id: COMPACTION_TEST_SESSION_ID.to_owned(),
        parent_session_id: None,
        created_at_ms: 100,
        startup_timeline: crate::run_commands::run_startup_snapshot::test_startup_timeline(),
        cwd: Some("E:\\fixture".to_owned()),
        provider: "openai".to_owned(),
        model: "gpt-5".to_owned(),
        selected_skill_name: None,
        repo_context: None,
        session_context: None,
        durable_memory: None,
        prompt: "用户问题".to_owned(),
        local_images: Vec::new(),
        local_files: Vec::new(),
        response: "助手回答".to_owned(),
        assistant_part_id: agent_run_assistant_part_id(1),
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

fn fixture_thread_settings() -> ThreadSettings {
    ThreadSettings {
        cwd: "E:\\fixture".to_owned(),
        provider: "openai".to_owned(),
        model: "gpt-5".to_owned(),
        selected_skill_name: None,
    }
}

fn fixture_turn() -> Turn {
    Turn {
        turn_id: "turn-1".to_owned(),
        sequence: 0,
        status: TurnStatus::Completed,
        history_items: vec![
            ThreadHistoryItem::UserMessage {
                sequence: 0,
                part_id: "user-1".to_owned(),
                content: "用户问题".to_owned(),
                local_images: Vec::new(),
                local_files: Vec::new(),
                provider_metadata: None,
            },
            ThreadHistoryItem::AssistantMessage {
                sequence: 1,
                part_id: "assistant-1".to_owned(),
                content: "助手回答".to_owned(),
                provider_metadata: None,
            },
        ],
        token_usage_json: None,
        started_at_ms: 100,
        completed_at_ms: Some(200),
        interrupted_at_ms: None,
        failure: None,
    }
}

fn fixture_compaction(checkpoint_text: &str, recorded_at_ms: u64) -> ThreadCompaction {
    ThreadCompaction {
        history_json: "[]".to_owned(),
        through_sequence: 0,
        checkpoint_text: checkpoint_text.to_owned(),
        before_tokens: 10,
        after_tokens: 5,
        recorded_at_ms,
    }
}
