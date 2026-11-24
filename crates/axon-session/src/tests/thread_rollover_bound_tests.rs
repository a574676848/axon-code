//! 换窗投影的体量回归：checkpoint 不受历史总量支配。
//!
//! 真机故障是「单条 JSONL 记录超过记录体量硬上限」：换窗投影写的 checkpoint 承载整份
//! ThreadDetail，长会话必然越过 64 MiB。这里用真实存储复现同一形状，断言投影之后
//! 日志里**任何一条**记录都在内联阈值以内，而 `read_thread` 仍能还原完整视图。

use std::path::{Path, PathBuf};

use super::*;

/// 每轮正文体量：单轮就超过内联阈值，确保逐轮就触发外置。
const TURN_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
/// 常规用例的轮数：总量约 12 MiB，足以让 checkpoint 记录越过内联阈值。
const REGULAR_TURNS: usize = 6;
/// 手动用例的轮数：总量约 120 MiB，覆盖真机 100 MB 级会话。
const LARGE_TURNS: usize = 60;

fn temp_root(label: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-thread-rollover-{label}-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ))
}

/// 造一条有 `turns` 轮、每轮正文超过内联阈值的 thread，返回末条事件的序号。
fn build_thread(root: &Path, thread_id: &str, turns: usize) -> u64 {
    let store = JsonlThreadEventStore;
    let started = ThreadEventRecord {
        schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
        thread_id: thread_id.to_owned(),
        sequence: 0,
        recorded_at_ms: 1,
        event: ThreadEvent::ThreadStarted {
            session_root_id: thread_id.to_owned(),
            parent_thread_id: None,
            forked_from_thread_id: None,
            source: ThreadSource::CliRun,
            ephemeral: false,
            settings: ThreadSettings {
                cwd: ".".to_owned(),
                provider: "local".to_owned(),
                model: "model".to_owned(),
                selected_skill_name: None,
            },
        },
    };
    store.append(root, &started).expect("写入 thread 头");

    let mut sequence = 1_u64;
    for index in 0..turns {
        let turn = Turn {
            turn_id: format!("{thread_id}-turn-{index}"),
            sequence: u64::try_from(index).unwrap_or(u64::MAX),
            status: TurnStatus::Completed,
            history_items: vec![ThreadHistoryItem::AssistantMessage {
                sequence: 0,
                part_id: format!("assistant-{index}"),
                content: "字".repeat(TURN_PAYLOAD_BYTES),
                provider_metadata: None,
            }],
            token_usage_json: None,
            started_at_ms: sequence,
            completed_at_ms: Some(sequence),
            interrupted_at_ms: None,
            failure: None,
        };
        let record = ThreadEventRecord {
            schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
            thread_id: thread_id.to_owned(),
            sequence,
            recorded_at_ms: sequence,
            event: ThreadEvent::TurnCompleted { turn },
        };
        store.append(root, &record).expect("写入轮次");
        sequence = sequence.saturating_add(1);
    }
    sequence.saturating_sub(1)
}

/// 日志里所有记录都必须小于内联阈值，且日志总量不随历史线性膨胀。
fn assert_records_bounded(root: &Path, thread_id: &str) {
    let limit = crate::infrastructure::THREAD_EVENT_INLINE_BODY_LIMIT_BYTES;
    let paths = JsonlThreadEventStore::thread_shard_paths(root, thread_id).expect("列出分片");
    assert!(!paths.is_empty(), "投影之后必须仍有记录");
    let mut total_bytes = 0_usize;
    for path in paths {
        let bytes = std::fs::read(&path).expect("读取分片");
        total_bytes = total_bytes.saturating_add(bytes.len());
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            assert!(
                line.len() < limit,
                "分片 {} 存在超过内联阈值的记录：{} 字节",
                path.display(),
                line.len()
            );
        }
    }
    assert!(
        total_bytes < 2 * 1024 * 1024,
        "换窗后日志总量必须有界，实际 {total_bytes} 字节"
    );
}

fn compact_with_summary(root: &Path, thread_id: &str, last_sequence: u64) {
    let service = ThreadService::new(JsonlThreadEventStore);
    let compaction = ThreadCompaction {
        history_json: "[]".to_owned(),
        through_sequence: last_sequence.saturating_sub(1),
        checkpoint_text: "换窗交接摘要：本轮只验证记录体量有界。".to_owned(),
        recorded_at_ms: 10_000,
    };
    service
        .compact_thread(
            root,
            CompactThreadCommand {
                thread_id: thread_id.to_owned(),
                recorded_at_ms: 10_000,
                expected_sequence: last_sequence,
                compaction,
            },
        )
        .expect("换窗投影必须成功");
}

#[test]
fn checkpoint_projection_keeps_records_bounded() {
    let root = temp_root("bounded");
    let thread_id = "thread-rollover-bounded";
    let last_sequence = build_thread(&root, thread_id, REGULAR_TURNS);

    compact_with_summary(&root, thread_id, last_sequence);

    assert_records_bounded(&root, thread_id);
    let detail = ThreadService::new(JsonlThreadEventStore)
        .read_thread(&root, thread_id)
        .expect("投影之后必须可读");
    assert!(detail.compacted_history.is_some(), "投影必须落进读取视图");
    assert_eq!(
        detail.turns.len(),
        REGULAR_TURNS,
        "跨越投影点的轮次必须全部保留"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// 旧日志里的换窗读数（已下线）必须仍能解码。
///
/// 曾换过窗的历史会话在 `threadCompacted` / `threadCheckpoint` 里都带着
/// `beforeTokens` / `afterTokens`；这两个键消失后若解析失败，整条 thread 会读不出来。
#[test]
fn legacy_compaction_tokens_are_ignored_on_deserialize() {
    let legacy = serde_json::json!({
        "historyJson": "[]",
        "throughSequence": 7,
        "checkpointText": "旧摘要",
        "beforeTokens": 825_338,
        "afterTokens": 31_733,
        "recordedAtMs": 10_000,
    });
    let parsed: ThreadCompaction = serde_json::from_value(legacy).expect("旧日志必须可解码");
    assert_eq!(parsed.through_sequence, 7);
    assert_eq!(parsed.checkpoint_text, "旧摘要");

    let encoded = serde_json::to_value(&parsed).expect("新日志应可编码");
    assert!(
        encoded.get("beforeTokens").is_none(),
        "新写入不得再带换窗前读数"
    );
    assert!(
        encoded.get("afterTokens").is_none(),
        "新写入不得再带换窗后读数"
    );
}

/// 真机 100 MB 级会话的手动回归：默认忽略，按需
/// `cargo test -p axon-session --lib checkpoint_projection_handles -- --ignored` 执行。
#[test]
#[ignore = "写入约 120 MiB，手动执行以复现真机量级"]
fn checkpoint_projection_handles_hundreds_of_megabytes() {
    let root = temp_root("large");
    let thread_id = "thread-rollover-large";
    let last_sequence = build_thread(&root, thread_id, LARGE_TURNS);

    compact_with_summary(&root, thread_id, last_sequence);

    assert_records_bounded(&root, thread_id);
    let detail = ThreadService::new(JsonlThreadEventStore)
        .read_thread(&root, thread_id)
        .expect("大体量投影之后必须可读");
    assert!(detail.compacted_history.is_some());
    assert_eq!(detail.turns.len(), LARGE_TURNS);

    let _ = std::fs::remove_dir_all(root);
}
