use super::*;
use crate::{MemoryConfig, MemorySqliteStore};

const NAMESPACE: &str = "workspace:current";

fn temp_store(label: &str) -> MemorySqliteStore {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-consolidate-{label}-{}-{}",
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

fn insert_record(store: &MemorySqliteStore, id: u64, importance: u16, updated_at_ms: u64) {
    store
        .connection
        .execute(
            "INSERT INTO memory_records (
                memory_id, candidate_id, memory_kind, state, content_ref, namespace_ref,
                created_at_ms, updated_at_ms, last_active_at_ms,
                confidence_basis_points, importance_basis_points
             ) VALUES (?1, ?2, 'knowledge', 'strong', ?3, ?4, ?5, ?6, ?5, 8000, ?7)",
            rusqlite::params![
                i64::try_from(id).unwrap(),
                i64::try_from(id).unwrap(),
                format!("memory://topic/{id}"),
                NAMESPACE,
                i64::try_from(updated_at_ms).unwrap(),
                i64::try_from(updated_at_ms).unwrap(),
                i64::from(importance),
            ],
        )
        .unwrap();
}

/// 造 20 条同主题记忆，分成 4 组，每组 1 保留 + 4 吸收。
fn seed_twenty(store: &MemorySqliteStore) -> Vec<u64> {
    let ids = (1..=20).collect::<Vec<u64>>();
    for id in &ids {
        insert_record(store, *id, 8_000, 1_000 + *id);
    }
    ids
}

fn merge_plan(ids: &[u64]) -> String {
    let groups = [
        (ids[19], vec![ids[0], ids[1], ids[2], ids[3]]),
        (ids[18], vec![ids[4], ids[5], ids[6], ids[7]]),
        (ids[17], vec![ids[8], ids[9], ids[10], ids[11]]),
        (ids[16], vec![ids[12], ids[13], ids[14], ids[15]]),
    ];
    let ops = groups
        .iter()
        .map(|(keep, absorb)| {
            format!(
                r#"{{"action":"merge","keep":{keep},"absorb":[{}]}}"#,
                absorb
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"ops":[{ops}]}}"#)
}

#[test]
fn merge_only_plan_reduces_twenty_records_to_four() {
    let store = temp_store("merge");
    let ids = seed_twenty(&store);
    assert_eq!(store.active_memory_count(NAMESPACE).unwrap(), 20);

    let plan = parse_plan(&merge_plan(&ids), &ids).expect("合法计划");
    let report = apply_plan(&store, &plan, 10_000, true).unwrap();
    assert_eq!(report.merged_groups, 4);
    assert_eq!(report.absorbed, 16);

    let remaining = store.active_memory_count(NAMESPACE).unwrap();
    assert!(remaining <= 6, "合并后生效条数必须 ≤6，实际 {remaining}");
    assert_eq!(remaining, 4);
}

#[test]
fn every_absorbed_record_is_traceable_from_successor() {
    let store = temp_store("trace");
    let ids = seed_twenty(&store);
    let plan = parse_plan(&merge_plan(&ids), &ids).expect("合法计划");
    apply_plan(&store, &plan, 10_000, true).unwrap();

    let expected_groups = [
        (ids[19], vec![ids[0], ids[1], ids[2], ids[3]]),
        (ids[18], vec![ids[4], ids[5], ids[6], ids[7]]),
        (ids[17], vec![ids[8], ids[9], ids[10], ids[11]]),
        (ids[16], vec![ids[12], ids[13], ids[14], ids[15]]),
    ];
    for (keep, absorb) in expected_groups {
        let predecessors = store.supersession_predecessors(keep).unwrap();
        for old in absorb {
            assert!(
                predecessors.contains(&old),
                "旧条 {old} 必须能在 {keep} 的替代链上追溯，实际 {predecessors:?}"
            );
        }
    }
}

#[test]
fn newest_record_stays_active_after_merge() {
    let store = temp_store("newest");
    let ids = seed_twenty(&store);
    let plan = parse_plan(&merge_plan(&ids), &ids).expect("合法计划");
    apply_plan(&store, &plan, 10_000, true).unwrap();
    let archived: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM memory_records WHERE memory_id = ?1 AND archived_at_ms IS NOT NULL",
            rusqlite::params![i64::try_from(ids[19]).unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archived, 0, "保留条不得被归档");
}

#[test]
fn new_memory_intent_is_rejected_and_database_is_untouched() {
    let store = temp_store("new-intent");
    let ids = seed_twenty(&store);
    let raw = r#"{"ops":[{"action":"new","text":"重复内容"}]}"#;
    assert_eq!(
        parse_plan(raw, &ids),
        Err(PlanRejection::NewMemoryIntent),
        "含新建意图的整批必须丢弃"
    );
    assert_eq!(store.active_memory_count(NAMESPACE).unwrap(), 20);
}

#[test]
fn insert_intent_is_rejected_alongside_valid_ops() {
    let ids = vec![1_u64, 2];
    let raw =
        r#"{"ops":[{"action":"merge","keep":2,"absorb":[1]},{"action":"insert","text":"x"}]}"#;
    assert_eq!(parse_plan(raw, &ids), Err(PlanRejection::NewMemoryIntent));
}

#[test]
fn unknown_target_is_rejected() {
    let ids = vec![1_u64, 2];
    let raw = r#"{"ops":[{"action":"merge","keep":99,"absorb":[1]}]}"#;
    assert_eq!(parse_plan(raw, &ids), Err(PlanRejection::UnknownTarget(99)));
}

#[test]
fn empty_and_malformed_plans_are_rejected() {
    let ids = vec![1_u64];
    assert_eq!(parse_plan(r#"{"ops":[]}"#, &ids), Err(PlanRejection::Empty));
    assert!(matches!(
        parse_plan("not json", &ids),
        Err(PlanRejection::Malformed(_))
    ));
    assert!(matches!(
        parse_plan(r#"{"ops":[{"action":"merge","keep":1}]}"#, &ids),
        Err(PlanRejection::Malformed(_))
    ));
}

#[test]
fn disabled_flag_writes_nothing() {
    let store = temp_store("disabled");
    let ids = seed_twenty(&store);
    let plan = parse_plan(&merge_plan(&ids), &ids).expect("合法计划");
    let report = apply_plan(&store, &plan, 10_000, false).unwrap();
    assert_eq!(report, ConsolidationReport::default());
    assert_eq!(store.active_memory_count(NAMESPACE).unwrap(), 20);
    // 常量断言写成 const 块：运行期没有任何信息量，编译期失败才是它的作用。
    const { assert!(CONSOLIDATE_ENABLED_DEFAULT, "记忆整合常量默认必须开启") };
}

#[test]
fn downgrade_halves_importance_with_floor() {
    let store = temp_store("downgrade");
    insert_record(&store, 1, 3_000, 1_000);
    assert!(store.downgrade_importance(1).unwrap());
    assert_eq!(read_importance(&store, 1), 1_500);
    assert!(store.downgrade_importance(1).unwrap());
    assert_eq!(read_importance(&store, 1), MIN_IMPORTANCE_BP);
    assert!(
        !store.downgrade_importance(1).unwrap(),
        "已到下限后不再变化"
    );
}

fn read_importance(store: &MemorySqliteStore, id: u64) -> u16 {
    let value: i64 = store
        .connection
        .query_row(
            "SELECT importance_basis_points FROM memory_records WHERE memory_id = ?1",
            rusqlite::params![i64::try_from(id).unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    u16::try_from(value).unwrap()
}

#[test]
fn batch_limit_is_twenty() {
    assert_eq!(CONSOLIDATE_BATCH_LIMIT, 20);
}
