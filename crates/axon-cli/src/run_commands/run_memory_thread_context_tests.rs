use std::path::{Path, PathBuf};

use super::{LOCAL_USER_REF, RunMemoryThreadInput, open_store, prepare_memory_thread_context};
use axon_config::AxonConfig;
use axon_memory::{COMMITMENT_STATUS_OPEN, MemoryCommitment, MemoryThread, THREAD_STATUS_OPEN};

const NAMESPACE: &str = "workspace:thread-test";
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

/// 预置一条前台线索与一条未闭合承诺，模拟历史轮次已经写入的数据。
fn seed_existing_state(root: &Path, config: &AxonConfig) {
    let store = open_store(root, config).expect("打开 Memory store");
    store
        .upsert_thread(&MemoryThread {
            thread_id: "thread-seed".to_owned(),
            namespace_ref: NAMESPACE.to_owned(),
            user_ref: LOCAL_USER_REF.to_owned(),
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
            namespace_ref: NAMESPACE.to_owned(),
            user_ref: LOCAL_USER_REF.to_owned(),
            text: "明天提醒我交周报".to_owned(),
            status: COMMITMENT_STATUS_OPEN.to_owned(),
            channel: "test".to_owned(),
            created_at_ms: 1_000,
            closed_at_ms: None,
        })
        .expect("写入承诺");
}

#[test]
fn empty_store_yields_no_context() {
    let root = temp_root("empty");
    std::fs::create_dir_all(root.join("data").join("memory")).expect("创建 Memory 目录");
    let config = AxonConfig::default();

    let output = prepare_memory_thread_context(&RunMemoryThreadInput {
        axon_root: &root,
        config: &config,
        namespace_ref: NAMESPACE,
        tz_offset_min: TZ_OFFSET_MIN,
    });

    assert_eq!(output.context, None, "空库不得凭空生成线索段");
    assert_eq!(output.recorded_commitment, None);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn renders_existing_thread_and_open_commitment() {
    let root = temp_root("seeded");
    std::fs::create_dir_all(root.join("data").join("memory")).expect("创建 Memory 目录");
    let config = AxonConfig::default();
    seed_existing_state(&root, &config);

    let output = prepare_memory_thread_context(&RunMemoryThreadInput {
        axon_root: &root,
        config: &config,
        namespace_ref: NAMESPACE,
        tz_offset_min: TZ_OFFSET_MIN,
    });
    let context = output.context.expect("应渲染既有线索与承诺");

    assert!(context.contains("## 当前线索"));
    assert!(context.contains("命中 3 次"));
    assert!(context.contains("## 未完成承诺"));
    assert!(context.contains("明天提醒我交周报"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn read_is_side_effect_free_across_repeated_calls() {
    let root = temp_root("repeatable");
    std::fs::create_dir_all(root.join("data").join("memory")).expect("创建 Memory 目录");
    let config = AxonConfig::default();
    seed_existing_state(&root, &config);
    let input = RunMemoryThreadInput {
        axon_root: &root,
        config: &config,
        namespace_ref: NAMESPACE,
        tz_offset_min: TZ_OFFSET_MIN,
    };

    let first = prepare_memory_thread_context(&input);
    let second = prepare_memory_thread_context(&input);

    assert_eq!(first, second, "重复调用必须产出逐字节一致的上下文段");
    let _ = std::fs::remove_dir_all(root);
}
