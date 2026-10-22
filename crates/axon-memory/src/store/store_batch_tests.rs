use super::*;

const CONCURRENT_WRITER_COUNT: usize = 8;
const CONCURRENT_RECORDS_PER_WRITER: usize = 32;
const PERFORMANCE_RECORD_COUNT: usize = 100_000;
const PERFORMANCE_SAMPLE_COUNT: usize = 100;

#[test]
fn sqlite_numeric_overflow_is_a_memory_persistence_error() {
    let error = i64_from_u64(u64::MAX).unwrap_err();

    assert!(matches!(
        error,
        AxonError::Memory(axon_domain::MemoryError::Persistence(message))
            if message == "Memory SQLite 数值溢出"
    ));
}

#[test]
fn batch_record_lookup_accepts_more_than_one_sql_batch() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-batch-records-{}-{}",
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
    let ids = (1..=MEMORY_SQLITE_QUERY_BATCH_SIZE + 1)
        .map(|value| MemoryId(value as u64))
        .collect::<Vec<_>>();

    assert!(store.records_by_memory_ids(&ids).unwrap().is_empty());

    drop(store);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn memory_kind_round_trips_through_sqlite() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-kind-roundtrip-{}-{}",
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
    let kinds = [
        (axon_domain::MemoryKind::Profile, Some("user:test")),
        (axon_domain::MemoryKind::Preference, Some("user:test")),
        (axon_domain::MemoryKind::Experience, None),
        (axon_domain::MemoryKind::Knowledge, None),
        (axon_domain::MemoryKind::Procedure, None),
    ];
    for (offset, (kind, user_ref)) in kinds.into_iter().enumerate() {
        let mut record = scoped_record(100 + offset as u64, user_ref, "namespace:test");
        record.memory_kind = kind;
        store.upsert_record(&record).unwrap();
        assert_eq!(
            store
                .get_record(record.record.id)
                .unwrap()
                .unwrap()
                .memory_kind,
            kind
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

use super::test_fixtures::{memory_summaries, personal_context_record, scoped_record};

#[test]
fn concurrent_writers_preserve_records_and_scope_isolation() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-concurrent-writers-{}-{}",
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
    let mut workers = Vec::with_capacity(CONCURRENT_WRITER_COUNT);
    for worker_id in 0..CONCURRENT_WRITER_COUNT {
        let worker_config = config.clone();
        workers.push(std::thread::spawn(move || {
            let store = MemorySqliteStore::open(&worker_config).unwrap();
            store.initialize_schema().unwrap();
            for offset in 0..CONCURRENT_RECORDS_PER_WRITER {
                let id = (worker_id * CONCURRENT_RECORDS_PER_WRITER + offset + 1) as u64;
                let namespace = if worker_id.is_multiple_of(2) {
                    "workspace:even"
                } else {
                    "workspace:odd"
                };
                let mut record = scoped_record(id, None, namespace);
                record.updated_at_ms = id;
                record.last_active_at_ms = id;
                store.upsert_record(&record).unwrap();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }

    let store = MemorySqliteStore::open(&config).unwrap();
    let expected = CONCURRENT_WRITER_COUNT * CONCURRENT_RECORDS_PER_WRITER;
    assert_eq!(store.summary().unwrap().total_records, expected);
    assert_eq!(
        store
            .recallable_records_by_namespace("workspace:even", expected as u64, expected)
            .unwrap()
            .len(),
        expected / 2
    );
    assert_eq!(
        store
            .recallable_records_by_namespace("workspace:odd", expected as u64, expected)
            .unwrap()
            .len(),
        expected / 2
    );
    drop(store);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "显式性能验收基准，不纳入常规门禁"]
fn memory_kind_counts_p95_with_one_hundred_thousand_records() {
    use std::time::{Duration, Instant};

    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-kind-performance-{}-{}",
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
    let transaction = store.connection.unchecked_transaction().unwrap();
    for id in 1..=PERFORMANCE_RECORD_COUNT {
        let mut record = scoped_record(id as u64, None, "workspace:benchmark");
        record.memory_kind = match id % 5 {
            0 => MemoryKind::Experience,
            1 => MemoryKind::Knowledge,
            _ => MemoryKind::Procedure,
        };
        record.updated_at_ms = id as u64;
        record.last_active_at_ms = id as u64;
        crate::store_write::upsert_record_on_connection(&transaction, &record).unwrap();
    }
    transaction.commit().unwrap();

    let mut samples = Vec::with_capacity(PERFORMANCE_SAMPLE_COUNT);
    for _ in 0..PERFORMANCE_SAMPLE_COUNT {
        let started = Instant::now();
        let counts = store
            .memory_kind_counts(MemoryVisibility::All, Some("workspace:benchmark"))
            .unwrap();
        assert_eq!(
            counts.iter().map(|item| item.count).sum::<usize>(),
            PERFORMANCE_RECORD_COUNT
        );
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p95 = samples[(PERFORMANCE_SAMPLE_COUNT * 95 / 100).saturating_sub(1)];
    eprintln!(
        "memory_kind_counts records={} samples={} p95_ms={:.3}",
        PERFORMANCE_RECORD_COUNT,
        PERFORMANCE_SAMPLE_COUNT,
        duration_millis(p95)
    );
    assert!(
        p95 < Duration::from_millis(100),
        "统计 p95 超过 100ms: {p95:?}"
    );
    drop(store);
    let _ = std::fs::remove_dir_all(root);
}

fn duration_millis(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[test]
fn personal_context_query_is_semantic_and_deterministic() {
    const OWNER: &str = "owner-a";
    const PERSONAL: &str = "personal:owner-a";
    const OTHER_OWNER: &str = "owner-b";
    const OTHER_PERSONAL: &str = "personal:owner-b";
    const QUERY_NOW_MS: u64 = 20;

    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-personal-context-{}-{}",
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
            OWNER,
            PERSONAL,
            MemorySourceOrigin::ExplicitUserPreference,
            "偏好中文",
        ),
        personal_context_record(
            2,
            OWNER,
            PERSONAL,
            MemorySourceOrigin::AgentUserContext,
            "默认称呼长官",
        ),
        personal_context_record(
            3,
            OWNER,
            PERSONAL,
            MemorySourceOrigin::AgentExperience,
            "仅属于 Workspace 的经验",
        ),
        personal_context_record(
            4,
            OTHER_OWNER,
            OTHER_PERSONAL,
            MemorySourceOrigin::ExplicitUserPreference,
            "其他 owner 的偏好",
        ),
    ] {
        store.upsert_record(&record).unwrap();
    }

    let first = store
        .personal_context_records_by_scope(OWNER, PERSONAL, QUERY_NOW_MS, 8)
        .unwrap();
    assert_eq!(
        memory_summaries(&first),
        vec!["偏好中文".to_owned(), "默认称呼长官".to_owned()]
    );

    let mut changed_metadata = personal_context_record(
        1,
        OWNER,
        PERSONAL,
        MemorySourceOrigin::ExplicitUserPreference,
        "偏好中文",
    );
    changed_metadata.updated_at_ms = 10;
    changed_metadata.last_active_at_ms = 10;
    changed_metadata.importance_basis_points = 1_000;
    store.upsert_record(&changed_metadata).unwrap();
    let second = store
        .personal_context_records_by_scope(OWNER, PERSONAL, QUERY_NOW_MS, 8)
        .unwrap();

    assert_eq!(memory_summaries(&first), memory_summaries(&second));
    drop(store);
    let _ = std::fs::remove_dir_all(root);
}
