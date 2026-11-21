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
fn oversize_event_is_externalized_instead_of_occupying_a_shard() {
    let root = temp_root("oversize");
    let store = JsonlThreadEventStore;
    store.append(&root, &record("oversize-thread", 0)).unwrap();
    assert!(JsonlThreadEventStore::shard_path(&root, "oversize-thread", 0).exists());

    let mut oversize = record("oversize-thread", 1);
    oversize.event = ThreadEvent::ThreadNamed {
        name: Some("x".repeat(INLINE_LIMIT * 4)),
    };
    store
        .append(&root, &oversize)
        .expect("超阈值事件应外置正文并写进引用，而不是被拒绝");

    assert_eq!(
        store.read_thread(&root, "oversize-thread").unwrap().len(),
        2,
        "两条记录都必须保留"
    );
    let shard = std::fs::read_to_string(JsonlThreadEventStore::shard_path(
        &root,
        "oversize-thread",
        0,
    ))
    .expect("读取分片");
    assert!(
        shard.lines().all(|line| line.len() < INLINE_LIMIT),
        "任何一条记录都不得越过内联阈值"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn event_beyond_hard_record_limit_is_externalized_not_rejected() {
    // 正文体量超过 64 MiB 硬上限不再意味着写不进去：日志行只承载引用，正文落独立文件。
    let root = temp_root("hard-limit");
    let store = JsonlThreadEventStore;
    let mut event = record("hard-limit-thread", 0);
    event.event = ThreadEvent::ThreadNamed {
        name: Some("x".repeat(usize::try_from(MAX_SINGLE_RECORD_BYTES).unwrap() / 2)),
    };

    store
        .append(&root, &event)
        .expect("正文超硬上限时应外置而不是拒绝整笔写入");
    let restored = store.read_thread(&root, "hard-limit-thread").unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].event, event.event, "外置后必须还原成同一事件");

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

const INLINE_LIMIT: usize = super::super::thread_event_body::THREAD_EVENT_INLINE_BODY_LIMIT_BYTES;

/// 超阈值事件追加后，分片里的那一行必须是引用而不是正文。
#[test]
fn oversized_append_keeps_the_shard_line_bounded() {
    let root = temp_root("oversized-append");
    let store = JsonlThreadEventStore;
    let thread_id = "thread-oversized-append";
    let mut record = thread_started(thread_id, 0);
    record.event = ThreadEvent::ThreadNamed {
        name: Some("注".repeat(INLINE_LIMIT * 2)),
    };

    store.append(&root, &record).expect("超阈值事件应可写入");

    let shard = std::fs::read_to_string(JsonlThreadEventStore::shard_path(&root, thread_id, 0))
        .expect("读取分片");
    let line = shard.lines().next().expect("分片应有一行");
    assert!(
        line.len() < INLINE_LIMIT,
        "日志行必须小于内联阈值，实际 {} 字节",
        line.len()
    );
    assert!(
        line.contains(super::super::thread_event_body::THREAD_EVENT_BODY_ARCHIVED_TYPE),
        "日志行必须是正文引用：{line}"
    );
    let body =
        super::super::thread_event_body::body_root(&root).join(body_relative_path(thread_id, 0));
    assert!(body.is_file(), "正文文件必须落盘：{}", body.display());
}

/// checkpoint 重写只留一条有界记录，旧正文随之回收。
#[test]
fn checkpoint_rewrite_keeps_one_bounded_line_and_prunes_old_bodies() {
    let root = temp_root("checkpoint-prune");
    let store = JsonlThreadEventStore;
    let thread_id = "thread-checkpoint-prune";
    let mut first = thread_started(thread_id, 0);
    first.event = ThreadEvent::ThreadNamed {
        name: Some("注".repeat(INLINE_LIMIT * 2)),
    };
    store.append(&root, &first).expect("首条事件应可写入");
    let body_root = super::super::thread_event_body::body_root(&root);
    let first_body = body_root.join(body_relative_path(thread_id, 0));
    assert!(first_body.is_file(), "首条正文应已落盘");

    let mut checkpoint = thread_started(thread_id, 1);
    checkpoint.event = ThreadEvent::ThreadNamed {
        name: Some("换".repeat(INLINE_LIMIT * 2)),
    };
    store
        .replace_thread_with_checkpoint(&root, &checkpoint, 0)
        .expect("checkpoint 重写应成功");

    let shard = std::fs::read_to_string(JsonlThreadEventStore::shard_path(&root, thread_id, 0))
        .expect("读取分片");
    assert_eq!(shard.lines().count(), 1, "重写后只应有一条记录");
    assert!(
        shard.lines().next().map(str::len).unwrap_or(usize::MAX) < INLINE_LIMIT,
        "重写后的记录必须有界"
    );
    assert!(!first_body.exists(), "被替换事件的正文必须回收");
    assert!(
        body_root.join(body_relative_path(thread_id, 1)).is_file(),
        "checkpoint 自己的正文必须保留"
    );
}

/// 管理型重写同样走外置，并只保留新记录集引用到的正文。
#[test]
fn management_rewrite_externalizes_and_keeps_referenced_bodies() {
    let root = temp_root("management-rewrite");
    let store = JsonlThreadEventStore;
    let thread_id = "thread-management-rewrite";
    let mut first = thread_started(thread_id, 0);
    first.event = ThreadEvent::ThreadNamed {
        name: Some("注".repeat(INLINE_LIMIT * 2)),
    };
    store.append(&root, &first).expect("首条事件应可写入");

    let mut second = thread_started(thread_id, 1);
    second.event = ThreadEvent::ThreadNamed {
        name: Some("二".repeat(INLINE_LIMIT * 2)),
    };
    store
        .rewrite_thread_with_records(&root, thread_id, 0, &[second])
        .expect("管理型重写应成功");

    let body_root = super::super::thread_event_body::body_root(&root);
    assert!(
        !body_root.join(body_relative_path(thread_id, 0)).exists(),
        "不再被引用的正文必须回收"
    );
    assert!(
        body_root.join(body_relative_path(thread_id, 1)).is_file(),
        "仍被引用的正文必须保留"
    );
}

/// 正文文件的相对路径与编码入口的命名保持一致。
fn body_relative_path(thread_id: &str, sequence: u64) -> String {
    format!(
        "{}/{:020}.json.zst",
        JsonlThreadEventStore::thread_prefix(thread_id),
        sequence
    )
}
