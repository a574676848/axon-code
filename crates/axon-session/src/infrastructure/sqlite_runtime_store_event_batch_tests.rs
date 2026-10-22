//! SQLite Session Event 批量追加测试。

use super::SqliteRuntimeStore;
use crate::{RuntimeStore, RuntimeStoreError};
use axon_domain::{DaemonSessionId, RuntimeHostId, SessionRuntimeStatus};
use std::path::PathBuf;

const HOST_CLAIMED_AT_MS: u64 = 10;
const HOST_LEASE_MS: u64 = 1_000;
const SINGLE_EVENT_AT_MS: u64 = 20;
const BATCH_EVENT_AT_MS: u64 = 30;

struct EventBatchFixture {
    database_path: PathBuf,
    store: SqliteRuntimeStore,
    host: crate::DaemonHostRecord,
    session_id: DaemonSessionId,
    // 字段顺序即析构顺序：临时目录必须最后析构，`store` 先释放 SQLite 句柄，目录才删得掉。
    _directory: axon_test_support::TestTempDir,
}

fn event_batch_fixture(name: &str) -> EventBatchFixture {
    let directory = axon_test_support::test_temp_dir();
    let database_path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new(&database_path);
    let host_id = RuntimeHostId::new(format!("host-{name}")).expect("host");
    let host = store
        .claim_host(&host_id, 1, None, HOST_CLAIMED_AT_MS, HOST_LEASE_MS)
        .expect("claim host");
    let session_id = DaemonSessionId::new(format!("session-{name}")).expect("session");
    store
        .create_session(&session_id, ".", HOST_CLAIMED_AT_MS)
        .expect("create session");
    store
        .set_session_status(
            &session_id,
            &host,
            SessionRuntimeStatus::Running,
            HOST_CLAIMED_AT_MS,
        )
        .expect("bind session");
    EventBatchFixture {
        database_path,
        store,
        host,
        session_id,
        _directory: directory,
    }
}

#[test]
fn batch_append_assigns_contiguous_sequences_and_updates_runtime_once() {
    let fixture = event_batch_fixture("sequence");
    let first = fixture
        .store
        .append_event(
            &fixture.session_id,
            &fixture.host,
            "provider.connected",
            "{\"connected\":true}",
            SINGLE_EVENT_AT_MS,
        )
        .expect("append single event");
    let events = vec![
        ("assistant.delta".to_owned(), "{\"text\":\"甲\"}".to_owned()),
        ("assistant.delta".to_owned(), "{\"text\":\"乙\"}".to_owned()),
        ("reasoning.delta".to_owned(), "{\"text\":\"丙\"}".to_owned()),
    ];

    let records = fixture
        .store
        .append_events(
            &fixture.session_id,
            &fixture.host,
            &events,
            BATCH_EVENT_AT_MS,
        )
        .expect("append event batch");

    assert_eq!(first.sequence, 1);
    assert_eq!(
        records
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert_eq!(
        records
            .iter()
            .map(|record| (record.event_type.clone(), record.payload_json.clone()))
            .collect::<Vec<_>>(),
        events
    );
    let runtime = fixture
        .store
        .get_session(&fixture.session_id)
        .expect("read runtime");
    assert_eq!(runtime.last_event_sequence, 4);
    assert_eq!(runtime.last_activity_at_ms, BATCH_EVENT_AT_MS);
    let empty_events = Vec::new();
    assert!(
        fixture
            .store
            .append_events(&fixture.session_id, &fixture.host, &empty_events, u64::MAX,)
            .expect("empty batch is a no-op")
            .is_empty()
    );
    assert_eq!(
        fixture
            .store
            .replay_events(&fixture.session_id, 0, 10)
            .expect("replay events")
            .len(),
        4
    );
}

#[test]
fn batch_append_rejects_stale_generation_without_partial_writes() {
    let fixture = event_batch_fixture("fenced");
    let stale_host = crate::DaemonHostRecord {
        generation: "stale-generation".to_owned(),
        ..fixture.host.clone()
    };
    let events = vec![
        ("assistant.delta".to_owned(), "{\"text\":\"甲\"}".to_owned()),
        ("assistant.delta".to_owned(), "{\"text\":\"乙\"}".to_owned()),
    ];

    let error = fixture
        .store
        .append_events(&fixture.session_id, &stale_host, &events, BATCH_EVENT_AT_MS)
        .expect_err("stale generation must be fenced");

    assert!(matches!(error, RuntimeStoreError::Fenced));
    assert!(
        fixture
            .store
            .replay_events(&fixture.session_id, 0, 10)
            .expect("replay events")
            .is_empty()
    );
}

#[test]
fn batch_append_rolls_back_all_events_when_later_insert_fails() {
    let fixture = event_batch_fixture("rollback");
    let connection = rusqlite::Connection::open(&fixture.database_path).expect("open database");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_batch_event_insert
             BEFORE INSERT ON session_events
             WHEN NEW.event_type = 'test.reject'
             BEGIN
                 SELECT RAISE(ABORT, 'reject batch event');
             END;",
        )
        .expect("create failure trigger");
    drop(connection);
    let events = vec![
        (
            "assistant.delta".to_owned(),
            "{\"text\":\"durable\"}".to_owned(),
        ),
        ("test.reject".to_owned(), "{}".to_owned()),
    ];

    fixture
        .store
        .append_events(
            &fixture.session_id,
            &fixture.host,
            &events,
            BATCH_EVENT_AT_MS,
        )
        .expect_err("second insert must abort the whole batch");

    assert!(
        fixture
            .store
            .replay_events(&fixture.session_id, 0, 10)
            .expect("replay events")
            .is_empty()
    );
    let runtime = fixture
        .store
        .get_session(&fixture.session_id)
        .expect("read runtime");
    assert_eq!(runtime.last_event_sequence, 0);
}
