//! 长会话治理回归：thread 窗口重写、事件正文分级与 WAL 截断。
//!
//! 三个用例都按「先算账、后执行」走一遍：dry-run 与执行态必须读出同一批对象，
//! 且 dry-run 不得改写任何文件。

use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_session::ports::EventLogStore as _;
use axon_session::{
    AXON_RUNTIME_DATA_DIR, AppendTurnCommand, CompactThreadCommand, JsonlThreadEventStore,
    RuntimeStore, SqliteRuntimeStore, StartEmptyThreadCommand, ThreadCompaction, ThreadHistoryItem,
    ThreadService, ThreadSettings, ThreadSource, Turn, TurnStatus,
};

use super::*;

/// 单轮正文体量：25 轮里有 5 轮是大轮，合计超过 4 MiB 的预筛下限。
const BIG_TURN_TEXT: usize = 1_024 * 1_024;
/// 事件载荷体量：41 条交互合计超过 2 MiB 的日志预筛下限。
const EVENT_TEXT: usize = 128 * 1_024;
/// 交互条数：保留窗口是 40，因此必须超出它才有可分级正文。
const INTERACTIONS: usize = 41;

/// 高熵正文：重复字符会被 zstd 压到几百字节，夹具就达不到日志预筛下限。
fn noise(seed: u64, len: usize) -> String {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    let mut text = String::with_capacity(len);
    for _ in 0..len {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // 64 值字母表：每位约 6 bit 熵，压缩后仍远大于阈值。
        text.push(char::from(b'a' + ((state >> 33) & 0x3F) as u8));
    }
    text
}

fn settings() -> ThreadSettings {
    ThreadSettings {
        cwd: ".".to_owned(),
        provider: "provider-a".to_owned(),
        model: "model-a".to_owned(),
        selected_skill_name: None,
    }
}

fn turn(sequence: u64, text: String) -> Turn {
    Turn {
        turn_id: format!("turn-{sequence}"),
        sequence,
        status: TurnStatus::Completed,
        history_items: vec![ThreadHistoryItem::UserMessage {
            sequence: 0,
            part_id: format!("part-{sequence}"),
            content: text,
            local_images: Vec::new(),
            local_files: Vec::new(),
            provider_metadata: None,
        }],
        token_usage_json: None,
        started_at_ms: 1_000 + sequence,
        completed_at_ms: Some(2_000 + sequence),
        interrupted_at_ms: None,
        failure: None,
    }
}

/// 建一条长 thread：25 轮，前 5 轮各 1 MiB，并写一份覆盖前 5 轮的压缩快照。
fn seed_long_thread(root: &Path) {
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_empty_thread(
            root,
            StartEmptyThreadCommand {
                thread_id: "thread-long".to_owned(),
                session_root_id: "thread-long".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 10,
            },
        )
        .expect("建 thread");
    for sequence in 0..25u64 {
        let text = if sequence < 5 {
            "x".repeat(BIG_TURN_TEXT)
        } else {
            "小轮".to_owned()
        };
        service
            .append_turn_at_tail(
                root,
                AppendTurnCommand {
                    thread_id: "thread-long".to_owned(),
                    recorded_at_ms: 100 + sequence,
                    turn: turn(sequence, text),
                },
            )
            .expect("追加轮次");
    }
    service
        .compact_thread(
            root,
            CompactThreadCommand {
                thread_id: "thread-long".to_owned(),
                recorded_at_ms: 900,
                expected_sequence: 25,
                compaction: ThreadCompaction {
                    history_json: "[]".to_owned(),
                    through_sequence: 5,
                    checkpoint_text: "已压缩".to_owned(),
                    before_tokens: 1,
                    after_tokens: 1,
                    recorded_at_ms: 900,
                },
            },
        )
        .expect("写压缩快照");
}

fn thread_bytes(root: &Path) -> u64 {
    JsonlThreadEventStore::thread_shard_paths(root, "thread-long")
        .expect("分片路径")
        .iter()
        .map(|path| std::fs::metadata(path).map_or(0, |meta| meta.len()))
        .sum()
}

/// 把文件修改时间回拨到很久以前：治理有静默期守卫，夹具必须表现为「已经停写」。
fn age_file(path: &Path) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("夹具文件应可打开");
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .expect("应可回拨修改时间");
}

/// 把一条 thread 的全部分片回拨修改时间。
fn age_thread(root: &Path) {
    for path in JsonlThreadEventStore::thread_shard_paths(root, "thread-long").expect("分片路径")
    {
        age_file(&path);
    }
}

#[test]
fn thread_window_rewrite_is_budgeted_and_dry_run_safe() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path();
    seed_long_thread(root);
    age_thread(root);
    let before = thread_bytes(root);
    assert!(
        before > THREAD_WINDOW_MIN_BYTES,
        "夹具必须超过预筛下限，实际 {before} 字节"
    );

    let dry = govern_thread_windows(root, true, &GovernanceBudget::new(true)).expect("算账");
    assert_eq!(dry.0, 1, "只有一条 thread 需要重写");
    assert_eq!(dry.1, 5, "压缩快照覆盖的前五轮应被移出");
    assert_eq!(thread_bytes(root), before, "dry-run 不得改写 thread 文件");

    let applied = govern_thread_windows(root, false, &GovernanceBudget::new(false)).expect("重写");
    assert_eq!(applied.0, 1);
    let after = thread_bytes(root);
    assert!(after < before, "重写后必须变小：{before} -> {after}");

    let detail = ThreadService::new(JsonlThreadEventStore)
        .read_thread(root, "thread-long")
        .expect("读回");
    assert_eq!(detail.turns.len(), 20, "窗口保留最近 20 轮");
    assert_eq!(detail.turns[0].sequence, 5, "快照之后的第一轮必须留下");
    assert!(detail.compacted_history.is_some(), "压缩快照必须保留");
}

#[test]
fn event_body_tiering_moves_old_bodies_and_reports_reclaimed_bytes() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path();
    let store = SqliteRuntimeStore::from_root(root.join(AXON_RUNTIME_DATA_DIR));
    let host = store
        .claim_host(
            &axon_domain::RuntimeHostId::new("governance-host").expect("host"),
            1,
            None,
            1,
            1_000,
        )
        .expect("host lease");
    let session_id = DaemonSessionId::new("session-governance").expect("会话标识");
    store.create_session(&session_id, ".", 1).expect("建会话");
    for index in 0..INTERACTIONS {
        let interaction_id =
            axon_domain::InteractionId::new(format!("interaction-{index:02}")).expect("交互");
        let payload = serde_json::json!({
            "interactionId": interaction_id.as_str(),
            "kind": {
                "type": "textDelta",
                "payload": { "text": noise(u64::try_from(index).unwrap_or(0) + 1, EVENT_TEXT) },
            },
        });
        store
            .append_attributed_events(
                &session_id,
                &host,
                &interaction_id,
                &[("wire_event".to_owned(), payload.to_string())],
                100 + u64::try_from(index).unwrap_or(0),
            )
            .expect("追加事件");
    }
    let log_path = store.event_log(&session_id).path();
    age_file(&log_path);
    let log_bytes = std::fs::metadata(&log_path).map_or(0, |meta| meta.len());
    assert!(
        log_bytes > EVENT_BODY_MIN_LOG_BYTES,
        "夹具日志必须超过预筛下限，实际 {log_bytes} 字节"
    );

    let dry = tier_event_bodies(root, true, &GovernanceBudget::new(true)).expect("算账");
    assert_eq!(dry.0, 1, "只分级窗口之外的那一条交互");
    assert!(dry.1 >= EVENT_TEXT as u64, "dry-run 也必须给出回收字节");
    assert_eq!(
        std::fs::metadata(&log_path).map_or(0, |meta| meta.len()),
        log_bytes,
        "dry-run 不得改写日志"
    );

    let applied = tier_event_bodies(root, false, &GovernanceBudget::new(false)).expect("分级");
    assert_eq!(applied.0, dry.0);
    assert!(applied.1 > 0);
    let after = std::fs::metadata(&log_path).map_or(0, |meta| meta.len());
    assert!(
        after < log_bytes,
        "分级后日志必须变小：{log_bytes} -> {after}"
    );
    assert!(
        axon_session::body_archive_path(
            &store.event_archive_root(),
            session_id.as_str(),
            "interaction-00"
        )
        .is_file(),
        "被分级的正文必须落进归档"
    );
}

#[test]
fn wal_truncate_is_a_no_op_when_no_runtime_database_exists() {
    let directory = axon_test_support::test_temp_dir();
    truncate_wal(directory.path()).expect("没有运行态库时不得不报错");
    assert!(
        !directory
            .path()
            .join(AXON_RUNTIME_DATA_DIR)
            .join(axon_session::CONTROL_DATABASE_FILE_NAME)
            .exists(),
        "维护任务不得抢先建出一个没有 schema 的空库"
    );
}
