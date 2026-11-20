//! 轮次事实侧车表的读写语义。
//!
//! 这张表的意义是「打开会话不再读大记录」：因此这里固定三件事，缺一件就退化成慢路径。
//! 一是按轮落一行且可重复写入，二是最新一轮按收口时刻取，三是缺行返回空（由读侧兜底）。

use std::sync::atomic::{AtomicU64, Ordering};

use super::SqliteRuntimeStore;
use crate::ports::RuntimeStore as _;

/// 夹具目录序号：同一测试线程内多个用例共用临时根，路径必须逐用例区分。
static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn fixture() -> SqliteRuntimeStore {
    let directory = axon_test_support::test_temp_path(&format!(
        "thread-turn-facts-{}",
        FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    store.initialize().expect("初始化");
    store
}

fn facts(thread_id: &str, part: &str, recorded_at_ms: u64) -> crate::ThreadTurnFacts {
    crate::ThreadTurnFacts {
        thread_id: thread_id.to_owned(),
        assistant_part_id: part.to_owned(),
        recorded_at_ms,
        model_name: Some("model-a".to_owned()),
        reasoning_effort: Some("max".to_owned()),
        reflection_notice: None,
    }
}

#[test]
fn latest_turn_facts_follows_recorded_time_and_reads_only_that_row() {
    let store = fixture();
    store
        .upsert_thread_turn_facts(&facts("thread-1", "part-old", 1_000), 1_000)
        .expect("写旧轮");
    store
        .upsert_thread_turn_facts(&facts("thread-1", "part-new", 2_000), 2_000)
        .expect("写新轮");

    let latest = store
        .latest_thread_turn_facts("thread-1")
        .expect("读最新轮")
        .expect("必须有一行");
    assert_eq!(latest.assistant_part_id, "part-new");
    assert_eq!(latest.model_name.as_deref(), Some("model-a"));
    assert_eq!(latest.reasoning_effort.as_deref(), Some("max"));
    assert!(latest.reflection_notice.is_none());
}

#[test]
fn notice_patch_updates_in_place_and_never_inserts_a_half_row() {
    let store = fixture();
    store
        .upsert_thread_turn_facts(&facts("thread-1", "part-1", 1_000), 1_000)
        .expect("写轮次");

    store
        .update_thread_turn_notice("thread-1", "part-1", "记忆反思：新增 1 条", 1_100)
        .expect("补写提示");
    let latest = store
        .latest_thread_turn_facts("thread-1")
        .expect("读最新轮")
        .expect("必须有一行");
    assert_eq!(
        latest.reflection_notice.as_deref(),
        Some("记忆反思：新增 1 条")
    );
    assert_eq!(
        latest.model_name.as_deref(),
        Some("model-a"),
        "补写单个字段不得抹掉整行"
    );

    // 行不存在时不动任何东西：不会长出一条只有提示、其余全空的半截事实。
    store
        .update_thread_turn_notice("thread-1", "part-missing", "不该出现", 1_200)
        .expect("补写走空路径");
    assert_eq!(
        store
            .latest_thread_turn_facts("thread-1")
            .expect("读最新轮")
            .map(|row| row.assistant_part_id),
        Some("part-1".to_owned())
    );
}

#[test]
fn missing_thread_reads_as_absent() {
    let store = fixture();
    assert!(
        store
            .latest_thread_turn_facts("thread-unknown")
            .expect("缺行不是错误")
            .is_none(),
        "缺行必须显式为空，由读侧回落尾窗读"
    );
}
