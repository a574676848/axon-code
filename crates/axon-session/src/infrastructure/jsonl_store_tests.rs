//! Thread JSONL 事件存储的单元测试。
//!
//! 主文件贴 700 行硬限，夹具与用例独立成文；`#[path]` 挂回同一个模块，`super::*` 照常可见。

use super::*;
use crate::ThreadService;
use crate::domain::{
    ThreadEvent, ThreadEventRecord, ThreadSettings, ThreadSource, Turn, TurnStatus,
};

fn record(thread_id: &str, sequence: u64) -> ThreadEventRecord {
    ThreadEventRecord {
        schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
        thread_id: thread_id.to_owned(),
        sequence,
        recorded_at_ms: sequence,
        event: ThreadEvent::ThreadNamed { name: None },
    }
}

fn thread_started(thread_id: &str, sequence: u64) -> ThreadEventRecord {
    ThreadEventRecord {
        schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
        thread_id: thread_id.to_owned(),
        sequence,
        recorded_at_ms: sequence,
        event: ThreadEvent::ThreadStarted {
            session_root_id: thread_id.to_owned(),
            parent_thread_id: None,
            forked_from_thread_id: None,
            source: ThreadSource::CliRun,
            ephemeral: false,
            settings: ThreadSettings {
                cwd: "E:\\fixture".to_owned(),
                provider: "local".to_owned(),
                model: "model".to_owned(),
                selected_skill_name: None,
            },
        },
    }
}

fn active_turn(thread_id: &str, sequence: u64) -> ThreadEventRecord {
    ThreadEventRecord {
        schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
        thread_id: thread_id.to_owned(),
        sequence,
        recorded_at_ms: sequence,
        event: ThreadEvent::TurnStarted {
            turn: Turn {
                turn_id: format!("{thread_id}-active"),
                sequence: 0,
                status: TurnStatus::Active,
                history_items: Vec::new(),
                token_usage_json: None,
                started_at_ms: sequence,
                completed_at_ms: None,
                interrupted_at_ms: None,
                failure: None,
            },
        },
    }
}

fn temp_root(label: &str) -> std::path::PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-thread-{label}-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ))
}

/// 落盘文件名是编码后的 thread 前缀，这里从分片名反推同一前缀，保证临时文件命名口径一致。
fn rewrite_temporary(root: &Path, thread_id: &str) -> std::path::PathBuf {
    let shard = JsonlThreadEventStore::shard_path(root, thread_id, 0);
    let name = shard.file_name().unwrap().to_string_lossy().into_owned();
    let prefix = name.split('.').next().unwrap().to_owned();
    shard.with_file_name(format!("{prefix}.rewrite0.tmp"))
}

/// 零分片 + 残留临时文件是重写崩溃的现场：读侧先锁内自愈一次，不能永久报错。
#[test]
fn rewrite_debris_is_healed_instead_of_read_as_missing() {
    let root = temp_root("debris-heal");
    std::fs::create_dir_all(JsonlThreadEventStore::path(&root)).unwrap();
    let temporary = rewrite_temporary(&root, "debris");
    std::fs::write(&temporary, b"{}").unwrap();
    assert!(
        JsonlThreadEventStore::thread_has_unfinished_write(&root, "debris"),
        "临时文件必须被识别为未完成写入（{}）",
        temporary.display()
    );

    let read = ThreadService::new(JsonlThreadEventStore)
        .read_thread_optional(&root, "debris")
        .expect("残留临时文件必须先被自愈清掉，而不是永久报错");
    assert!(read.is_none(), "清掉现场后确实没有这条 thread");
    assert!(!temporary.exists(), "残留临时文件必须被清掉");
    assert!(!JsonlThreadEventStore::thread_has_unfinished_write(
        &root, "debris"
    ));
    let _ = std::fs::remove_dir_all(root);
}

/// 清不掉的未完成写入仍然必须报错：读侧不得把损坏的会话当成新会话重建。
#[test]
fn uncleaable_rewrite_debris_still_reports_incomplete() {
    let root = temp_root("debris-stuck");
    std::fs::create_dir_all(JsonlThreadEventStore::path(&root)).unwrap();
    let temporary = rewrite_temporary(&root, "debris");
    // 用目录占住临时文件名：它不是文件，`remove_file` 删不掉，自愈因此失败。
    std::fs::create_dir_all(&temporary).unwrap();

    let error = ThreadService::new(JsonlThreadEventStore)
        .read_thread_optional(&root, "debris")
        .expect_err("清不掉的未完成写入必须报错");
    assert!(error.contains("未完成重写"), "{error}");
    let _ = std::fs::remove_dir_all(root);
}

/// 分片序号出现缺口时读侧报不完整：安静地少一段历史比报错更危险。
#[test]
fn thread_shard_gap_reads_as_incomplete() {
    let root = temp_root("shard-gap");
    std::fs::create_dir_all(JsonlThreadEventStore::path(&root)).unwrap();
    std::fs::write(
        JsonlThreadEventStore::shard_path(&root, "gapped", 1),
        serde_json::to_string(&thread_started("gapped", 0)).unwrap() + "\n",
    )
    .unwrap();

    let error = JsonlThreadEventStore
        .read_thread(&root, "gapped")
        .expect_err("缺一块分片必须报不完整");
    assert!(error.contains("不完整"), "{error}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn activation_reads_latest_events_across_thread_shards() {
    let root = temp_root("activation");
    let store = JsonlThreadEventStore;
    std::fs::create_dir_all(JsonlThreadEventStore::path(&root)).unwrap();
    std::fs::write(
        JsonlThreadEventStore::shard_path(&root, "cross-shard", 0),
        serde_json::to_string(&thread_started("cross-shard", 0)).unwrap() + "\n",
    )
    .unwrap();
    std::fs::write(
        JsonlThreadEventStore::shard_path(&root, "cross-shard", 1),
        serde_json::to_string(&active_turn("cross-shard", 1)).unwrap() + "\n",
    )
    .unwrap();

    let activation = store.read_thread_activation(&root, "cross-shard").unwrap();
    let detail = ThreadService::new(store)
        .read_thread(&root, "cross-shard")
        .unwrap();

    assert_eq!(activation.next_sequence, 2);
    assert_eq!(
        activation.active_turn_id.as_deref(),
        Some("cross-shard-active")
    );
    assert_eq!(detail.turns.len(), 1);
    assert_eq!(detail.turns[0].turn_id, "cross-shard-active");
    assert_eq!(detail.turns[0].status, TurnStatus::Active);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn append_rolls_thread_jsonl_when_shard_exceeds_limit() {
    let root = temp_root("shard");
    let store = JsonlThreadEventStore;
    let first_shard = JsonlThreadEventStore::shard_path(&root, "sharded-thread", 0);
    std::fs::create_dir_all(first_shard.parent().unwrap()).unwrap();
    let mut first_record = record("sharded-thread", 0);
    first_record.event = ThreadEvent::ThreadNamed {
        name: Some(String::new()),
    };
    let baseline_length = serde_json::to_string(&first_record).unwrap().len() as u64;
    let name_length = usize::try_from(THREAD_SHARD_MAX_BYTES - baseline_length - 1).unwrap();
    first_record.event = ThreadEvent::ThreadNamed {
        name: Some("x".repeat(name_length)),
    };
    let first_line = serde_json::to_string(&first_record).unwrap();
    std::fs::write(&first_shard, first_line + "\n").unwrap();
    store.append(&root, &record("sharded-thread", 1)).unwrap();

    let records = store.read_thread(&root, "sharded-thread").unwrap();
    assert_eq!(
        records.iter().map(|item| item.sequence).collect::<Vec<_>>(),
        [0, 1]
    );
    assert!(JsonlThreadEventStore::shard_path(&root, "sharded-thread", 1).exists());

    store
        .replace_thread_with_checkpoint(&root, &record("sharded-thread", 1), 1)
        .unwrap();
    assert!(!JsonlThreadEventStore::shard_path(&root, "sharded-thread", 1).exists());
    assert!(JsonlThreadEventStore::shard_path(&root, "sharded-thread", 0).exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn oversize_event_occupies_its_own_shard() {
    let root = temp_root("oversize");
    let store = JsonlThreadEventStore;
    store.append(&root, &record("oversize-thread", 0)).unwrap();
    assert!(JsonlThreadEventStore::shard_path(&root, "oversize-thread", 0).exists());

    let mut oversize = record("oversize-thread", 1);
    oversize.event = ThreadEvent::ThreadNamed {
        name: Some("x".repeat(usize::try_from(THREAD_SHARD_MAX_BYTES).unwrap() + 1_024)),
    };
    store
        .append(&root, &oversize)
        .expect("超过分片预算的单条事件应独占新分片，而不是被拒绝");

    assert!(
        JsonlThreadEventStore::shard_path(&root, "oversize-thread", 1).exists(),
        "超预算事件必须落进下一个分片"
    );
    assert_eq!(
        store.read_thread(&root, "oversize-thread").unwrap().len(),
        2,
        "两条记录都必须保留"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn rejects_single_event_beyond_hard_record_limit() {
    let root = temp_root("hard-limit");
    let store = JsonlThreadEventStore;
    let mut event = record("hard-limit-thread", 0);
    event.event = ThreadEvent::ThreadNamed {
        name: Some("x".repeat(usize::try_from(MAX_SINGLE_RECORD_BYTES).unwrap() + 1)),
    };

    assert!(
        store.append(&root, &event).is_err(),
        "超过单条记录硬上限必须拒绝写入"
    );
    assert!(!JsonlThreadEventStore::path(&root).exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn rewrite_thread_with_records_replaces_and_clears_all_shards() {
    let root = temp_root("rewrite");
    let store = JsonlThreadEventStore;
    let first = record("rewrite-thread", 1);
    let second = record("rewrite-thread", 2);
    store.append(&root, &first).unwrap();
    store.append(&root, &second).unwrap();
    std::fs::create_dir_all(JsonlThreadEventStore::path(&root)).unwrap();
    std::fs::write(
        JsonlThreadEventStore::shard_path(&root, "rewrite-thread", 1),
        format!("{}\n", serde_json::to_string(&second).unwrap()),
    )
    .unwrap();
    assert!(JsonlThreadEventStore::shard_path(&root, "rewrite-thread", 1).exists());

    store
        .rewrite_thread_with_records(&root, "rewrite-thread", 2, std::slice::from_ref(&first))
        .unwrap();
    assert_eq!(
        store.read_thread(&root, "rewrite-thread").unwrap(),
        vec![first]
    );
    assert!(!JsonlThreadEventStore::shard_path(&root, "rewrite-thread", 1).exists());

    store
        .rewrite_thread_with_records(&root, "rewrite-thread", 1, &[])
        .unwrap();
    assert!(
        store
            .read_thread(&root, "rewrite-thread")
            .unwrap()
            .is_empty()
    );
    assert!(!JsonlThreadEventStore::shard_path(&root, "rewrite-thread", 0).exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn long_thread_ids_use_bounded_stable_paths() {
    let thread_id =
        "task-session-task-1234567890--subagent--root_task-task-session-task-1234567890".repeat(8);
    let first = JsonlThreadEventStore::thread_prefix(&thread_id);
    let second = JsonlThreadEventStore::thread_prefix(&thread_id);

    assert_eq!(first, second);
    assert!(first.starts_with("thread-"));
    assert!(first.len() <= 71);
}

#[test]
fn short_thread_ids_keep_legacy_paths() {
    assert_eq!(
        JsonlThreadEventStore::thread_prefix("short-thread"),
        "73686f72742d746872656164"
    );
}
