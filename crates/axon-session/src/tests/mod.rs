use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use axon_domain::RuntimeFailure;

use crate::{
    AXON_RUNTIME_DATA_DIR, AppendTurnItemsCommand, CompactThreadCommand, FailTurnCommand,
    ForkThreadCommand, InterruptTurnCommand, JsonlThreadEventStore, NameThreadCommand,
    ResumeThreadCommand, SESSION_STORE_DIR, StartEmptyThreadCommand, StartThreadCommand,
    StartTurnCommand, ThreadActivationSnapshot, ThreadCompaction, ThreadEvent, ThreadEventRecord,
    ThreadEventStore as _, ThreadHistoryItem, ThreadHistoryStatus, ThreadService, ThreadSettings,
    ThreadSource, Turn, TurnStatus, session_store_path,
};

#[derive(Debug, Clone, Default)]
pub(super) struct TargetThreadOnlyStore {
    read_count: Arc<AtomicUsize>,
    appended: Arc<Mutex<Vec<ThreadEventRecord>>>,
}

impl TargetThreadOnlyStore {
    pub(super) fn read_count(&self) -> usize {
        self.read_count.load(Ordering::Relaxed)
    }

    fn appended(&self) -> Vec<ThreadEventRecord> {
        self.appended.lock().expect("测试事件锁不应中毒").clone()
    }
}

impl crate::ThreadEventStore for TargetThreadOnlyStore {
    fn append(&self, _root: &std::path::Path, record: &ThreadEventRecord) -> Result<(), String> {
        self.appended
            .lock()
            .map_err(|_| "测试事件锁异常".to_owned())?
            .push(record.clone());
        Ok(())
    }

    fn read_all(&self, _root: &std::path::Path) -> Result<Vec<ThreadEventRecord>, String> {
        Err("目标 thread 快速路径禁止全量读取".to_owned())
    }

    fn read_thread(
        &self,
        _root: &std::path::Path,
        thread_id: &str,
    ) -> Result<Vec<ThreadEventRecord>, String> {
        self.read_count.fetch_add(1, Ordering::Relaxed);
        Ok(vec![ThreadEventRecord {
            schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
            thread_id: thread_id.to_owned(),
            sequence: 0,
            recorded_at_ms: 100,
            event: ThreadEvent::ThreadStarted {
                session_root_id: thread_id.to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
            },
        }])
    }
}

pub(super) fn root(name: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-session-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

pub(super) fn settings() -> ThreadSettings {
    ThreadSettings {
        cwd: "E:\\fixture".to_owned(),
        provider: "local".to_owned(),
        model: "small".to_owned(),
        selected_skill_name: None,
    }
}

pub(super) fn turn(id: &str, sequence: u64, status: TurnStatus) -> Turn {
    Turn {
        turn_id: id.to_owned(),
        sequence,
        status,
        history_items: vec![
            ThreadHistoryItem::UserMessage {
                sequence: 0,
                part_id: format!("user-{id}"),
                content: format!("prompt-{id}"),
                local_images: Vec::new(),
                local_files: Vec::new(),
                provider_metadata: None,
            },
            ThreadHistoryItem::AssistantMessage {
                sequence: 1,
                part_id: format!("assistant-{id}"),
                content: format!("response-{id}"),
                provider_metadata: None,
            },
        ],
        token_usage_json: None,
        started_at_ms: 1000 + sequence,
        completed_at_ms: (status == TurnStatus::Completed).then_some(1000 + sequence),
        interrupted_at_ms: None,
        failure: None,
    }
}

#[test]
fn activation_snapshot_matches_projection_and_supports_fast_recovery() {
    let root = root("activation-fast-path");
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: "thread-activation".to_owned(),
                session_root_id: "thread-activation".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 100,
            },
        )
        .unwrap();
    service
        .start_turn(
            &root,
            StartTurnCommand {
                thread_id: "thread-activation".to_owned(),
                recorded_at_ms: 110,
                turn: turn("active", 0, TurnStatus::Active),
            },
        )
        .unwrap();

    let snapshot = service
        .read_thread_activation(&root, "thread-activation")
        .unwrap();
    assert!(snapshot.exists);
    assert_eq!(snapshot.next_sequence, 2);
    assert_eq!(snapshot.active_turn_id.as_deref(), Some("active"));

    service
        .interrupt_turn(
            &root,
            InterruptTurnCommand {
                thread_id: "thread-activation".to_owned(),
                turn_id: "active".to_owned(),
                recorded_at_ms: 120,
            },
        )
        .unwrap();
    let snapshot = service
        .read_thread_activation(&root, "thread-activation")
        .unwrap();

    assert_eq!(
        snapshot,
        ThreadActivationSnapshot {
            exists: true,
            next_sequence: 3,
            active_turn_id: None,
        }
    );
    assert_eq!(
        service
            .read_thread(&root, "thread-activation")
            .unwrap()
            .turns[0]
            .status,
        TurnStatus::Interrupted
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn resume_thread_reads_target_shard_once_and_appends_next_sequence() {
    let root = root("resume-target-thread");
    let store = TargetThreadOnlyStore::default();
    ThreadService::new(store.clone())
        .resume_thread(
            &root,
            &ResumeThreadCommand {
                thread_id: "thread".to_owned(),
                recorded_at_ms: 200,
            },
        )
        .unwrap();

    assert_eq!(store.read_count(), 1);
    let appended = store.appended();
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].sequence, 1);
    assert!(matches!(appended[0].event, ThreadEvent::ThreadResumed));
}

#[test]
fn compact_thread_replaces_jsonl_history_with_checkpoint_and_keeps_sequence_contiguous() {
    let root = root("jsonl-checkpoint");
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_thread(
            &root,
            StartThreadCommand {
                thread_id: "thread".to_owned(),
                session_root_id: "thread".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 100,
                turn: turn("turn", 0, TurnStatus::Completed),
            },
        )
        .unwrap();
    service
        .compact_thread(
            &root,
            CompactThreadCommand {
                thread_id: "thread".to_owned(),
                recorded_at_ms: 200,
                expected_sequence: 1,
                compaction: ThreadCompaction {
                    history_json: "[]".to_owned(),
                    through_sequence: 0,
                    checkpoint_text: "摘要".to_owned(),
                    before_tokens: 10,
                    after_tokens: 1,
                    recorded_at_ms: 200,
                },
            },
        )
        .unwrap();

    let records = JsonlThreadEventStore.read_thread(&root, "thread").unwrap();
    assert_eq!(records.len(), 1);
    assert!(matches!(
        records[0].event,
        ThreadEvent::ThreadCheckpoint { .. }
    ));
    assert_eq!(service.read_thread(&root, "thread").unwrap().turns.len(), 1);

    service
        .name_thread(
            &root,
            NameThreadCommand {
                thread_id: "thread".to_owned(),
                name: Some("已压缩".to_owned()),
                recorded_at_ms: 300,
            },
        )
        .unwrap();
    let records = JsonlThreadEventStore.read_thread(&root, "thread").unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].sequence, records[0].sequence + 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn compact_thread_rejects_stale_history_version_without_overwriting_records() {
    let root = root("jsonl-checkpoint-conflict");
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_thread(
            &root,
            StartThreadCommand {
                thread_id: "thread".to_owned(),
                session_root_id: "thread".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 100,
                turn: turn("turn", 0, TurnStatus::Completed),
            },
        )
        .unwrap();
    let error = service
        .compact_thread(
            &root,
            CompactThreadCommand {
                thread_id: "thread".to_owned(),
                recorded_at_ms: 200,
                expected_sequence: 0,
                compaction: ThreadCompaction {
                    history_json: "[]".to_owned(),
                    through_sequence: 0,
                    checkpoint_text: "摘要".to_owned(),
                    before_tokens: 10,
                    after_tokens: 1,
                    recorded_at_ms: 200,
                },
            },
        )
        .unwrap_err();

    assert!(error.contains("version conflict"));
    let records = JsonlThreadEventStore.read_thread(&root, "thread").unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| !matches!(record.event, ThreadEvent::ThreadCheckpoint { .. }))
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn tolerant_scan_isolates_invalid_shard_without_weakening_strict_reads() {
    let root = root("tolerant-scan");
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: "valid-thread".to_owned(),
                session_root_id: "valid-thread".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 100,
            },
        )
        .unwrap();
    let event_directory = JsonlThreadEventStore::path(&root);
    std::fs::create_dir_all(&event_directory).unwrap();
    std::fs::write(event_directory.join("broken.jsonl"), "{}\n").unwrap();

    let scan = JsonlThreadEventStore.scan_all(&root).unwrap();

    assert!(!scan.records.is_empty());
    assert_eq!(scan.issues.len(), 1);
    assert_eq!(scan.issues[0].shard, "broken.jsonl");
    assert!(JsonlThreadEventStore.read_all(&root).is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ephemeral_thread_does_not_write_long_term_store() {
    let root = root("ephemeral");
    let service = ThreadService::new(JsonlThreadEventStore);

    service
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: "ephemeral-thread".to_owned(),
                session_root_id: "ephemeral-thread".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: true,
                settings: settings(),
                created_at_ms: 100,
            },
        )
        .unwrap();

    assert!(JsonlThreadEventStore.read_all(&root).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fork_interrupts_active_source_turn_before_copying_visible_history() {
    let root = root("fork-active");
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_thread(
            &root,
            StartThreadCommand {
                thread_id: "parent".to_owned(),
                session_root_id: "parent".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 100,
                turn: turn("done", 0, TurnStatus::Completed),
            },
        )
        .unwrap();
    service
        .start_turn(
            &root,
            StartTurnCommand {
                thread_id: "parent".to_owned(),
                recorded_at_ms: 110,
                turn: turn("active", 1, TurnStatus::Active),
            },
        )
        .unwrap();

    service
        .fork_thread(
            &root,
            ForkThreadCommand {
                source_thread_id: "parent".to_owned(),
                new_thread_id: "child".to_owned(),
                settings: None,
                ephemeral: false,
                recorded_at_ms: 120,
            },
        )
        .unwrap();

    let parent = service.read_thread(&root, "parent").unwrap();
    let child = service.read_thread(&root, "child").unwrap();
    assert_eq!(parent.turns[1].status, TurnStatus::Interrupted);
    assert_eq!(child.thread.parent_thread_id.as_deref(), Some("parent"));
    assert_eq!(child.turns.len(), 2);
    assert_eq!(child.turns[1].status, TurnStatus::Interrupted);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn active_turn_persists_incremental_items_and_projects_errored_terminal() {
    let root = root("turn-delta");
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: "thread-delta".to_owned(),
                session_root_id: "thread-delta".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 100,
            },
        )
        .unwrap();
    service
        .start_turn(
            &root,
            StartTurnCommand {
                thread_id: "thread-delta".to_owned(),
                recorded_at_ms: 110,
                turn: turn("active-delta", 0, TurnStatus::Active),
            },
        )
        .unwrap();
    service
        .append_turn_items(
            &root,
            AppendTurnItemsCommand {
                thread_id: "thread-delta".to_owned(),
                turn_id: "active-delta".to_owned(),
                items: vec![ThreadHistoryItem::ToolCall {
                    sequence: 2,
                    provider_turn_id: 0,
                    part_id: "tool-part".to_owned(),
                    tool_call_id: "1".to_owned(),
                    operation_id: "operation-1".to_owned(),
                    provider_call_id: "call-1".to_owned(),
                    tool_name: "builtin.read_file".to_owned(),
                    input: r#"{"path":"Cargo.toml"}"#.into(),
                    status: ThreadHistoryStatus::Completed,
                    provider_metadata: None,
                }],
                recorded_at_ms: 120,
            },
        )
        .unwrap();
    let failure = RuntimeFailure::new(
        "provider_protocol",
        "provider_tool_call_incomplete",
        "provider failed",
        130,
    );
    service
        .fail_turn(
            &root,
            FailTurnCommand {
                thread_id: "thread-delta".to_owned(),
                turn_id: "active-delta".to_owned(),
                failure: failure.clone(),
                recorded_at_ms: 130,
            },
        )
        .unwrap();

    let records = JsonlThreadEventStore.read_all(&root).unwrap();
    assert_eq!(records.len(), 4);
    assert!(matches!(
        records[0].event,
        ThreadEvent::ThreadStarted { .. }
    ));
    assert!(matches!(records[1].event, ThreadEvent::TurnStarted { .. }));
    assert!(matches!(
        &records[2].event,
        ThreadEvent::TurnItemsAppended { items, .. } if items.len() == 1
    ));
    assert!(matches!(
        &records[3].event,
        ThreadEvent::TurnFailed { failure: actual, .. } if actual == &failure
    ));
    let detail = service.read_thread(&root, "thread-delta").unwrap();
    assert_eq!(detail.turns.len(), 1);
    assert_eq!(detail.turns[0].history_items.len(), 3);
    assert_eq!(detail.turns[0].status, TurnStatus::Failed);
    assert_eq!(detail.turns[0].failure.as_ref(), Some(&failure));
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn list_thread_page_matches_windows_verbatim_cwd() {
    let root = root("windows-verbatim-cwd");
    let service = ThreadService::new(JsonlThreadEventStore);
    let mut verbatim_settings = settings();
    verbatim_settings.cwd = r"\\?\E:\fixture".to_owned();
    service
        .start_thread(
            &root,
            StartThreadCommand {
                thread_id: "thread-verbatim".to_owned(),
                session_root_id: "thread-verbatim".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: verbatim_settings,
                created_at_ms: 100,
                turn: turn("done", 0, TurnStatus::Completed),
            },
        )
        .unwrap();

    let page = service
        .list_thread_page(
            &root,
            &crate::ThreadListQuery {
                limit: 10,
                cwd: Some("e:/fixture/".to_owned()),
                ..crate::ThreadListQuery::default()
            },
        )
        .unwrap();

    assert_eq!(page.total_records, 1);
    assert_eq!(page.threads[0].thread_id, "thread-verbatim");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn thread_name_projects_to_detail_and_summary() {
    let root = root("thread-name");
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_thread(
            &root,
            StartThreadCommand {
                thread_id: "thread".to_owned(),
                session_root_id: "thread".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 100,
                turn: turn("done", 0, TurnStatus::Completed),
            },
        )
        .unwrap();
    service
        .name_thread(
            &root,
            NameThreadCommand {
                thread_id: "thread".to_owned(),
                recorded_at_ms: 110,
                name: Some("TUI 标题生成".to_owned()),
            },
        )
        .unwrap();

    let detail = service.read_thread(&root, "thread").unwrap();
    let summary = service.list_threads(&root, 1).unwrap().remove(0);

    assert_eq!(detail.thread.name.as_deref(), Some("TUI 标题生成"));
    assert_eq!(summary.name.as_deref(), Some("TUI 标题生成"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn session_persistence_paths_share_data_directory() {
    let root = PathBuf::from("axon-root");
    let sessions = root.join(AXON_RUNTIME_DATA_DIR).join(SESSION_STORE_DIR);

    assert_eq!(session_store_path(&root), sessions);
    assert_eq!(
        JsonlThreadEventStore::path(&root).parent(),
        Some(sessions.as_path())
    );
}
