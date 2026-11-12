use super::*;
use crate::store_thread::COMMITMENT_STATUS_OPEN;
use crate::thread::ensure_foreground;
use crate::{MemoryConfig, MemorySqliteStore};

fn temp_store(label: &str) -> MemorySqliteStore {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-commit-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let config = MemoryConfig::from_config_dir(
        &root,
        false,
        crate::MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
    )
    .unwrap();
    std::fs::create_dir_all(config.db_path.parent().unwrap()).unwrap();
    let store = MemorySqliteStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    store
}

const SCOPE: ThreadScope<'static> = ThreadScope {
    namespace_ref: "workspace:current",
    user_ref: "user:local",
};

#[test]
fn chinese_commitment_phrase_is_detected() {
    assert!(detects_commitment("明天提醒我交周报"));
    assert!(detects_commitment("回头给你看一下"));
    assert!(detects_commitment("我记下了"));
}

#[test]
fn english_commitment_phrase_is_detected() {
    assert!(detects_commitment("Remind me to send the report"));
}

#[test]
fn ordinary_sentence_is_not_commitment() {
    assert!(!detects_commitment("今天天气怎么样"));
    assert!(!detects_commitment(""));
}

#[test]
fn closure_phrase_is_detected_only_in_short_sentence() {
    assert!(detects_closure("周报交了"));
    assert!(detects_closure("搞定"));
    assert!(detects_closure("done"));
    assert!(
        !detects_closure("好了我们先把构建脚本改成并行执行再补上重试逻辑"),
        "长句里的「好了」不应关闭承诺"
    );
}

#[test]
fn promise_is_written_as_open_commitment() {
    let store = temp_store("open");
    let thread_id = ensure_foreground(&store, &SCOPE, "明天提醒我交周报", 100, true).unwrap();
    let id = record_commitment(
        &store,
        &SCOPE,
        thread_id.as_deref(),
        "明天提醒我交周报",
        "cli",
        100,
        true,
    )
    .unwrap()
    .expect("应写入承诺");
    let open = store
        .open_commitments(SCOPE.namespace_ref, SCOPE.user_ref, 5)
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].commitment_id, id);
    assert_eq!(open[0].status, COMMITMENT_STATUS_OPEN);
    assert_eq!(open[0].channel, "cli");
}

#[test]
fn same_thread_promise_updates_instead_of_adding() {
    let store = temp_store("singleton");
    let thread_id = ensure_foreground(&store, &SCOPE, "明天提醒我交周报", 100, true).unwrap();
    record_commitment(&store, &SCOPE, thread_id.as_deref(), "明天提醒我交周报", "cli", 100, true)
        .unwrap();
    record_commitment(
        &store,
        &SCOPE,
        thread_id.as_deref(),
        "明天提醒我交周报和日报",
        "cli",
        200,
        true,
    )
    .unwrap();
    assert_eq!(store.commitment_count().unwrap(), 1);
    let open = store
        .open_commitments(SCOPE.namespace_ref, SCOPE.user_ref, 5)
        .unwrap();
    assert_eq!(open[0].text, "明天提醒我交周报和日报");
}

#[test]
fn closure_turns_commitment_into_done() {
    let store = temp_store("close");
    record_commitment(&store, &SCOPE, None, "明天提醒我交周报", "cli", 100, true).unwrap();
    let closed = close_commitments(&store, &SCOPE, "周报交了", 200, true).unwrap();
    assert_eq!(closed, 1);
    assert!(
        store
            .open_commitments(SCOPE.namespace_ref, SCOPE.user_ref, 5)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.commitment_count().unwrap(), 1, "关闭不得删除行");
}

#[test]
fn disabled_flag_skips_detection_and_writes() {
    let store = temp_store("disabled");
    assert!(record_commitment(&store, &SCOPE, None, "明天提醒我交周报", "cli", 100, false)
        .unwrap()
        .is_none());
    assert_eq!(store.commitment_count().unwrap(), 0);
    assert_eq!(
        close_commitments(&store, &SCOPE, "搞定", 200, false).unwrap(),
        0
    );
}
