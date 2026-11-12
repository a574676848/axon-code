use super::*;
use crate::{MemoryConfig, MemorySqliteStore, THREAD_STATUS_OPEN};

fn temp_store(label: &str) -> MemorySqliteStore {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-fg-{label}-{}-{}",
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
fn five_anaphora_turns_keep_one_thread_id() {
    let store = temp_store("anaphora");
    let first = ensure_foreground(&store, &SCOPE, "帮我排查构建失败", 100, true)
        .unwrap()
        .expect("首轮应建立线索");
    let mut ids = vec![first];
    for (index, text) in ["这个", "它", "继续", "接着", "刚才"].iter().enumerate() {
        ids.push(
            ensure_foreground(&store, &SCOPE, text, 200 + index as i64, true)
                .unwrap()
                .expect("指代轮应沿用同一线索"),
        );
    }
    assert_eq!(store.thread_count().unwrap(), 1, "五轮指代只能有一条线索");
    assert!(
        ids.iter().all(|id| id == &ids[0]),
        "thread_id 必须恒定：{ids:?}"
    );
}

#[test]
fn anaphora_without_foreground_does_not_create_thread() {
    let store = temp_store("anaphora-empty");
    let id = ensure_foreground(&store, &SCOPE, "这个呢", 100, true).unwrap();
    assert!(id.is_none());
    assert_eq!(store.thread_count().unwrap(), 0);
}

#[test]
fn one_off_leaf_does_not_touch_foreground() {
    let store = temp_store("one-off");
    let first = ensure_foreground(&store, &SCOPE, "帮我排查构建失败", 100, true)
        .unwrap()
        .expect("应建立线索");
    assert!(
        ensure_foreground(&store, &SCOPE, "今天天气怎么样", 200, true)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.thread_count().unwrap(), 1);
    let fg = store
        .foreground_thread(SCOPE.namespace_ref, SCOPE.user_ref)
        .unwrap()
        .expect("前台线索应保持不变");
    assert_eq!(fg.thread_id, first);
    assert_eq!(fg.hit_count, 1, "一次性输入不得累加 hit_count");
    assert_eq!(fg.last_event_at_ms, 100);
}

#[test]
fn disabled_flag_writes_nothing() {
    let store = temp_store("disabled");
    assert!(
        ensure_foreground(&store, &SCOPE, "帮我排查构建失败", 100, false)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.thread_count().unwrap(), 0);
}

#[test]
fn normal_topic_continues_existing_foreground() {
    let store = temp_store("continue");
    let first = ensure_foreground(&store, &SCOPE, "帮我排查构建失败", 100, true)
        .unwrap()
        .expect("应建立线索");
    let second = ensure_foreground(&store, &SCOPE, "再看一下磁盘占用", 200, true)
        .unwrap()
        .expect("应沿用线索");
    assert_eq!(first, second);
    let fg = store
        .foreground_thread(SCOPE.namespace_ref, SCOPE.user_ref)
        .unwrap()
        .expect("前台线索应存在");
    assert_eq!(fg.hit_count, 2);
}

#[test]
fn archived_foreground_is_replaced_by_new_thread() {
    let store = temp_store("archived");
    let first = ensure_foreground(&store, &SCOPE, "帮我排查构建失败", 100, true)
        .unwrap()
        .expect("应建立线索");
    store.archive_thread(&first, 150).unwrap();
    let second = ensure_foreground(&store, &SCOPE, "换一个话题聊部署", 200, true)
        .unwrap()
        .expect("归档后应新建线索");
    assert_ne!(first, second);
    assert_eq!(store.thread_count().unwrap(), 2);
}

#[test]
fn thread_signature_is_namespace_scoped() {
    let store = temp_store("signature");
    let other = ThreadScope {
        namespace_ref: "workspace:other",
        user_ref: "user:local",
    };
    let left = ensure_foreground(&store, &SCOPE, "帮我排查构建失败", 100, true)
        .unwrap()
        .expect("应建立线索");
    let right = ensure_foreground(&store, &other, "帮我排查构建失败", 100, true)
        .unwrap()
        .expect("应建立线索");
    assert_ne!(left, right, "不同命名空间必须是不同线索");
}

#[test]
fn opened_thread_starts_open_with_single_hit() {
    let store = temp_store("shape");
    let id = ensure_foreground(&store, &SCOPE, "帮我排查构建失败", 100, true)
        .unwrap()
        .expect("应建立线索");
    let fg = store
        .foreground_thread(SCOPE.namespace_ref, SCOPE.user_ref)
        .unwrap()
        .expect("前台线索应存在");
    assert_eq!(fg.thread_id, id);
    assert_eq!(fg.status, THREAD_STATUS_OPEN);
    assert_eq!(fg.hit_count, 1);
    assert!(fg.signature.contains("workspace:current"));
    assert!(!fg.topic.is_empty());
}

#[test]
fn commitment_id_and_open_commitment_are_stable() {
    let store = temp_store("commitment");
    let id = ensure_foreground(&store, &SCOPE, "明天提醒我交周报", 100, true)
        .unwrap()
        .expect("应建立线索");
    let commitment = open_commitment(&SCOPE, Some(&id), "明天提醒我交周报", "cli", 100);
    store.upsert_open_commitment(&commitment).unwrap();
    let open = store
        .open_commitments(SCOPE.namespace_ref, SCOPE.user_ref, 5)
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].thread_id.as_deref(), Some(id.as_str()));
    assert_eq!(open[0].status, crate::COMMITMENT_STATUS_OPEN);

    store
        .close_commitment(
            &commitment.commitment_id,
            crate::COMMITMENT_STATUS_DONE,
            200,
        )
        .unwrap();
    assert!(
        store
            .open_commitments(SCOPE.namespace_ref, SCOPE.user_ref, 5)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn touch_foreground_bumps_hit_count() {
    let store = temp_store("touch");
    let id = ensure_foreground(&store, &SCOPE, "帮我排查构建失败", 100, true)
        .unwrap()
        .expect("应建立线索");
    assert!(touch_foreground(&store, &id, 300).unwrap());
    let fg = store
        .foreground_thread(SCOPE.namespace_ref, SCOPE.user_ref)
        .unwrap()
        .expect("前台线索应存在");
    assert_eq!(fg.hit_count, 2);
}
