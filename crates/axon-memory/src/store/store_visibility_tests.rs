use super::test_fixtures::{personal_context_record, scoped_record};
use super::*;

#[test]
fn personal_and_workspace_queries_are_strictly_scope_isolated() {
    const OWNER_A: &str = "owner-a";
    const OWNER_B: &str = "owner-b";
    const PERSONAL_A: &str = "personal:owner-a";
    const PERSONAL_B: &str = "personal:owner-b";
    const WORKSPACE_A: &str = "workspace:a";
    const WORKSPACE_B: &str = "workspace:b";

    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-scope-isolation-{}-{}",
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
    let store = MemorySqliteStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    for record in [
        scoped_record(1, Some(OWNER_A), PERSONAL_A),
        scoped_record(2, Some(OWNER_B), PERSONAL_B),
        scoped_record(3, None, WORKSPACE_A),
        scoped_record(4, None, WORKSPACE_B),
    ] {
        store.upsert_record(&record).unwrap();
    }

    let personal = store
        .recallable_records_by_scope(OWNER_A, PERSONAL_A, 2, 8)
        .unwrap();
    assert_eq!(
        personal
            .iter()
            .map(|record| record.record.id)
            .collect::<Vec<_>>(),
        vec![MemoryId(1)]
    );
    assert!(
        store
            .recallable_records_by_scope(OWNER_A, PERSONAL_B, 2, 8)
            .unwrap()
            .is_empty()
    );
    let workspace = store
        .recallable_records_by_namespace(WORKSPACE_A, 2, 8)
        .unwrap();
    assert_eq!(
        workspace
            .iter()
            .map(|record| record.record.id)
            .collect::<Vec<_>>(),
        vec![MemoryId(3)]
    );

    drop(store);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn repeated_initialization_keeps_archived_state_stable() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-schema-idempotent-{}-{}",
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
    let store = MemorySqliteStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    let mut record = scoped_record(21, None, "workspace:idempotent");
    record.memory_kind = MemoryKind::Knowledge;
    store.upsert_record(&record).unwrap();

    store.initialize_schema().unwrap();

    assert!(store.legacy_record_tables().unwrap().is_empty());
    assert_eq!(store.summary().unwrap().total_records, 1);
    assert_eq!(
        store.get_record(MemoryId(21)).unwrap().unwrap().memory_kind,
        MemoryKind::Knowledge
    );
    drop(store);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn owner_scoped_visibility_lists_every_real_scope_without_cross_owner_leaks() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-owner-visibility-{}-{}",
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
    let store = MemorySqliteStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    for record in [
        personal_context_record(
            1,
            "owner-a",
            "personal:owner-a",
            MemorySourceOrigin::ExplicitUserPreference,
            "owner-a 偏好",
        ),
        personal_context_record(
            2,
            "owner-b",
            "personal:owner-b",
            MemorySourceOrigin::ExplicitUserPreference,
            "owner-b 偏好",
        ),
        scoped_record(3, None, "workspace:registered"),
        scoped_record(4, None, "workspace:branch"),
    ] {
        store.upsert_record(&record).unwrap();
    }

    let kinds = store
        .memory_kind_counts(MemoryVisibility::OwnerScoped("owner-a"), None)
        .unwrap();
    assert_eq!(
        kinds.iter().map(|item| item.count).sum::<usize>(),
        3,
        "owner-a 可见面应为自身 Personal 加全部 Workspace，且不含 owner-b"
    );

    let facets = store
        .memory_scope_facets(MemoryVisibility::OwnerScoped("owner-a"))
        .unwrap();
    let labels = facets
        .iter()
        .map(|facet| {
            (
                facet.scope_kind,
                facet.namespace_ref.as_str(),
                facet.record_count,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            ("personal", "personal:owner-a", 1),
            ("workspace", "workspace:branch", 1),
            ("workspace", "workspace:registered", 1),
        ],
        "scope 面必须来自 SQLite 单次聚合，且与真实写入 namespace 一致"
    );

    let page = store
        .records_timeline_page(
            axon_domain::MemoryKind::Preference,
            None,
            MemoryVisibility::OwnerScoped("owner-a"),
            None,
            10,
        )
        .unwrap();
    assert_eq!(
        page.iter()
            .map(|record| record.record.id.0)
            .collect::<Vec<_>>(),
        vec![1],
        "owner-a 的类型时间轴不得出现 owner-b 的 Personal 记录"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(root);
}
#[test]
fn memory_kind_timeline_is_bounded_cursor_paged_and_indexed() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-kind-timeline-{}-{}",
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
    let store = MemorySqliteStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    for (id, kind, updated_at_ms) in [
        (11, axon_domain::MemoryKind::Preference, 100),
        (12, axon_domain::MemoryKind::Preference, 100),
        (13, axon_domain::MemoryKind::Preference, 90),
        (14, axon_domain::MemoryKind::Knowledge, 120),
    ] {
        let mut record = scoped_record(id, Some("user:test"), "personal:test");
        if kind == axon_domain::MemoryKind::Knowledge {
            record.user_ref = None;
            record.namespace_ref = Some("workspace:test".to_owned());
        }
        record.memory_kind = kind;
        record.created_at_ms = updated_at_ms;
        record.updated_at_ms = updated_at_ms;
        record.last_active_at_ms = updated_at_ms;
        store.upsert_record(&record).unwrap();
    }

    let first_page = store
        .records_timeline_page(
            axon_domain::MemoryKind::Preference,
            None,
            MemoryVisibility::All,
            None,
            2,
        )
        .unwrap();
    assert_eq!(
        first_page
            .iter()
            .map(|record| record.record.id.0)
            .collect::<Vec<_>>(),
        vec![11, 12]
    );
    let cursor = MemoryTimelineCursor {
        updated_at_ms: first_page[1].updated_at_ms,
        memory_id: first_page[1].record.id.0,
    };
    let second_page = store
        .records_timeline_page(
            axon_domain::MemoryKind::Preference,
            None,
            MemoryVisibility::All,
            Some(cursor),
            2,
        )
        .unwrap();
    assert_eq!(
        second_page
            .iter()
            .map(|record| record.record.id.0)
            .collect::<Vec<_>>(),
        vec![13]
    );

    let counts = store
        .memory_kind_counts(MemoryVisibility::All, None)
        .unwrap();
    assert!(counts.iter().any(|count| {
        count.memory_kind == axon_domain::MemoryKind::Preference && count.count == 3
    }));
    assert!(counts.iter().any(|count| {
        count.memory_kind == axon_domain::MemoryKind::Knowledge && count.count == 1
    }));

    let mut statement = store
        .connection
        .prepare("EXPLAIN QUERY PLAN SELECT * FROM memory_records WHERE memory_kind = 'preference' ORDER BY updated_at_ms DESC, memory_id ASC LIMIT 2")
        .unwrap();
    let plans = statement
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        plans
            .iter()
            .any(|plan| plan.contains("idx_memory_records_kind_timeline"))
    );

    drop(statement);
    drop(store);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scoped_kind_timeline_pages_hit_the_scope_kind_index() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-scope-kind-plan-{}-{}",
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
    let store = MemorySqliteStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    for id in 1..=40u64 {
        let mut record = scoped_record(id, None, "workspace:plan");
        record.memory_kind = axon_domain::MemoryKind::Knowledge;
        record.updated_at_ms = id;
        record.last_active_at_ms = id;
        store.upsert_record(&record).unwrap();
    }

    let page = store
        .records_timeline_page(
            axon_domain::MemoryKind::Knowledge,
            Some("workspace:plan"),
            MemoryVisibility::OwnerScoped("owner-a"),
            Some(MemoryTimelineCursor {
                updated_at_ms: 40,
                memory_id: 40,
            }),
            5,
        )
        .unwrap();
    assert_eq!(
        page.iter()
            .map(|record| record.record.id.0)
            .collect::<Vec<_>>(),
        vec![39, 38, 37, 36, 35]
    );

    let plans = explain_query_plan(
        &store,
        "SELECT * FROM memory_records WHERE memory_kind = 'knowledge' AND (user_ref IS NULL OR user_ref = 'owner-a') AND namespace_ref = 'workspace:plan' AND (updated_at_ms < 40 OR (updated_at_ms = 40 AND memory_id > 40)) ORDER BY updated_at_ms DESC, memory_id ASC LIMIT 5",
    );
    assert!(
        plans
            .iter()
            .any(|plan| plan.contains("idx_memory_records_scope_kind_timeline")),
        "真实 scope 时间轴查询必须命中 scope+kind 索引：{plans:?}"
    );
    assert!(
        !plans.iter().any(|plan| plan.contains("TEMP B-TREE")),
        "有界分页不得退化成无界临时排序：{plans:?}"
    );

    let all_scopes = explain_query_plan(
        &store,
        "SELECT * FROM memory_records WHERE memory_kind = 'knowledge' AND (user_ref IS NULL OR user_ref = 'owner-a') ORDER BY updated_at_ms DESC, memory_id ASC LIMIT 5",
    );
    assert!(
        all_scopes
            .iter()
            .any(|plan| plan.contains("idx_memory_records_kind_timeline")),
        "全 scope 时间轴必须命中 kind 时间轴索引：{all_scopes:?}"
    );
    assert!(
        !all_scopes.iter().any(|plan| plan.contains("TEMP B-TREE")),
        "全 scope 时间轴不得退化成无界临时排序：{all_scopes:?}"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(root);
}

fn explain_query_plan(store: &MemorySqliteStore, sql: &str) -> Vec<String> {
    let mut statement = store
        .connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap();
    let plans = statement
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    drop(statement);
    plans
}
