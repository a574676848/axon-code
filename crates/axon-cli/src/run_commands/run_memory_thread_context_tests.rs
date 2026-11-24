use std::path::{Path, PathBuf};

use super::{maintain_memory_thread, prepare_memory_thread_context, with_store};
use axon_config::AxonConfig;
use axon_memory::{COMMITMENT_STATUS_OPEN, MemoryCommitment, MemoryThread, THREAD_STATUS_OPEN};
use axon_memory_read::MEMORY_GLOBAL_NAMESPACE_REF;

const USER_REF: &str = "user:local";
const SESSION: &str = "session-a";
const TZ_OFFSET_MIN: i32 = 480;

fn temp_root(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    axon_test_support::test_temp_root().join(format!(
        "axon-cli-thread-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn fresh_root(label: &str) -> PathBuf {
    let root = temp_root(label);
    std::fs::create_dir_all(root.join("data").join("memory")).expect("创建 Memory 目录");
    root
}

/// 灌入一条历史线索与一条未闭合承诺，模拟之前轮次已经写入的数据。
fn seed_existing_state(root: &Path, config: &AxonConfig) {
    with_store(root, config, |store| {
        store
            .upsert_thread(&MemoryThread {
                thread_id: "thread-seed".to_owned(),
                namespace_ref: MEMORY_GLOBAL_NAMESPACE_REF.to_owned(),
                user_ref: USER_REF.to_owned(),
                session_ref: SESSION.to_owned(),
                topic: "把并发锁的释放位置改掉".to_owned(),
                signature: "signature-seed".to_owned(),
                label: String::new(),
                summary: String::new(),
                status: THREAD_STATUS_OPEN.to_owned(),
                hit_count: 3,
                last_event_at_ms: 1_000,
                created_at_ms: 1_000,
                updated_at_ms: 1_000,
            })
            .expect("写入线索");
        store
            .upsert_open_commitment(&MemoryCommitment {
                commitment_id: "commitment-seed".to_owned(),
                thread_id: Some("thread-seed".to_owned()),
                namespace_ref: MEMORY_GLOBAL_NAMESPACE_REF.to_owned(),
                user_ref: USER_REF.to_owned(),
                text: "明天提醒我交周报".to_owned(),
                status: COMMITMENT_STATUS_OPEN.to_owned(),
                channel: "test".to_owned(),
                created_at_ms: 1_000,
                closed_at_ms: None,
            })
            .expect("写入承诺");
    })
    .expect("打开 Memory store");
}

#[test]
fn empty_store_yields_no_context() {
    let root = fresh_root("empty");
    let config = AxonConfig::default();

    let context = prepare_memory_thread_context(&root, &config, SESSION, TZ_OFFSET_MIN);

    assert_eq!(context, None, "空库不得凭空生成线索段");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn seeded_thread_and_commitment_are_rendered() {
    let root = fresh_root("seeded");
    let config = AxonConfig::default();
    seed_existing_state(&root, &config);

    let context = prepare_memory_thread_context(&root, &config, SESSION, TZ_OFFSET_MIN)
        .expect("应渲染既有线索与承诺");

    assert!(context.contains("## 当前线索"));
    assert!(context.contains("命中 3 次"));
    assert!(context.contains("## 未完成承诺"));
    assert!(context.contains("明天提醒我交周报"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn maintain_writes_thread_and_open_commitment() {
    let root = fresh_root("maintain");
    let config = AxonConfig::default();

    let commitment = maintain_memory_thread(&root, &config, SESSION, "明天提醒我交周报", 1_000);
    assert!(commitment.is_some(), "承诺句式必须落库并返回承诺 ID");

    let context = prepare_memory_thread_context(&root, &config, SESSION, TZ_OFFSET_MIN)
        .expect("写入后本轮应能读到线索与承诺");
    assert!(context.contains("## 当前线索"));
    assert!(context.contains("## 未完成承诺"));
    assert!(context.contains("明天提醒我交周报"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn closure_sentence_closes_open_commitment() {
    let root = fresh_root("closure");
    let config = AxonConfig::default();

    maintain_memory_thread(&root, &config, SESSION, "明天提醒我交周报", 1_000);
    maintain_memory_thread(&root, &config, SESSION, "周报交了", 2_000);

    let context =
        prepare_memory_thread_context(&root, &config, SESSION, TZ_OFFSET_MIN).unwrap_or_default();
    assert!(
        !context.contains("## 未完成承诺"),
        "已闭合的承诺不得继续渲染：{context}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn read_is_side_effect_free_across_repeated_calls() {
    let root = fresh_root("repeatable");
    let config = AxonConfig::default();
    seed_existing_state(&root, &config);

    let first = prepare_memory_thread_context(&root, &config, SESSION, TZ_OFFSET_MIN);
    let second = prepare_memory_thread_context(&root, &config, SESSION, TZ_OFFSET_MIN);

    assert_eq!(first, second, "重复调用必须产出逐字节一致的上下文段");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn other_session_does_not_see_the_thread_segment() {
    let root = fresh_root("session-isolation");
    let config = AxonConfig::default();
    seed_existing_state(&root, &config);

    let other = prepare_memory_thread_context(&root, &config, "session-b", TZ_OFFSET_MIN)
        .unwrap_or_default();

    assert!(
        !other.contains("## 当前线索"),
        "其它会话不得读到本会话的线索：{other}"
    );
    let _ = std::fs::remove_dir_all(root);
}
