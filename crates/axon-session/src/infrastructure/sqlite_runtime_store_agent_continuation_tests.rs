//! `jarvis_continuations` 的落盘、幂等与「最多一次有效执行」测试
//! （合同 §5.3.3 / §8.2.2）。

use super::SqliteRuntimeStore;
use crate::{
    AgentContinuationAppendOutcome, AgentContinuationQuery, AgentContinuationRecord,
    AgentContinuationState, AgentContinuationStore,
};

const OWNER_ID: &str = "session-owner-1";
const PARENT_SESSION_ID: &str = "parent-session-1";
const CHILD_SESSION_ID: &str = "child-session-1";
const CREATED_AT_MS: u64 = 1_000;
const RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const ARTIFACT_PATH: &str = "E:\\axon\\data\\logs\\tee\\job-7.shell.log";

struct ContinuationFixture {
    database_path: std::path::PathBuf,
    store: SqliteRuntimeStore,
    // 字段顺序即析构顺序：临时目录必须最后析构，`store` 先释放 SQLite 句柄，目录才删得掉。
    _directory: axon_test_support::TestTempDir,
}

fn continuation_fixture() -> ContinuationFixture {
    let directory = axon_test_support::test_temp_dir();
    let database_path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new(&database_path);
    ContinuationFixture {
        database_path,
        store,
        _directory: directory,
    }
}

fn continuation(continuation_id: &str, key: &str) -> AgentContinuationRecord {
    AgentContinuationRecord::pending(
        continuation_id,
        key,
        OWNER_ID,
        PARENT_SESSION_ID,
        CHILD_SESSION_ID,
        "/root/review",
        "completed",
        "子代理正常结束",
        "审查完成：发现 2 处命名问题",
        CREATED_AT_MS,
        CREATED_AT_MS + RETENTION_MS,
    )
    .with_job_ref("epoch-1:7")
    .with_execution_ref("execution-3")
    .with_artifact_path(ARTIFACT_PATH)
    .with_tokens(120, 45, 8)
}

/// 直接打开底层库断言物理行数，避免只验证内存视图。
fn count_rows(store: &SqliteRuntimeStore, key: &str) -> i64 {
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    connection
        .query_row(
            "SELECT COUNT(*) FROM jarvis_continuations WHERE idempotency_key=?1",
            rusqlite::params![key],
            |row| row.get(0),
        )
        .expect("读取行数")
}

#[test]
fn continuation_round_trips_every_projection_field() {
    let fixture = continuation_fixture();
    let record = continuation("continuation-1", "continuation:child-session-1:completed");
    assert_eq!(
        fixture.store.append_continuation(&record).expect("落盘"),
        AgentContinuationAppendOutcome::Inserted
    );

    let loaded = fixture
        .store
        .continuation("continuation-1")
        .expect("读取")
        .expect("存在");
    assert_eq!(loaded, record);
    assert_eq!(loaded.state, AgentContinuationState::Pending);
    assert!(loaded.state.is_pending());
    assert_eq!(loaded.artifact_path.as_deref(), Some(ARTIFACT_PATH));
    assert_eq!(loaded.token_input, Some(120));
    assert_eq!(loaded.token_output, Some(45));
    assert_eq!(loaded.token_reasoning, Some(8));
    assert_eq!(loaded.execution_ref.as_deref(), Some("execution-3"));

    let by_key = fixture
        .store
        .continuation_by_idempotency_key("continuation:child-session-1:completed")
        .expect("读取")
        .expect("存在");
    assert_eq!(by_key.continuation_id, "continuation-1");
}

#[test]
fn duplicate_idempotency_key_writes_exactly_one_continuation() {
    let fixture = continuation_fixture();
    let first = continuation("continuation-1", "continuation:child-session-1:completed");
    assert_eq!(
        fixture.store.append_continuation(&first).expect("首次落盘"),
        AgentContinuationAppendOutcome::Inserted
    );

    // 同一子代理同一次执行的同一终态重复产生：只落一行，并回报既有状态。
    let duplicate = continuation("continuation-2", "continuation:child-session-1:completed");
    assert_eq!(
        fixture
            .store
            .append_continuation(&duplicate)
            .expect("重复落盘"),
        AgentContinuationAppendOutcome::Duplicate(AgentContinuationState::Pending)
    );
    assert_eq!(
        count_rows(&fixture.store, "continuation:child-session-1:completed"),
        1
    );
    assert!(
        fixture
            .store
            .continuation("continuation-2")
            .expect("读取")
            .is_none()
    );
}

#[test]
fn consumption_produces_at_most_one_effective_execution() {
    let fixture = continuation_fixture();
    fixture
        .store
        .append_continuation(&continuation("continuation-1", "key-1"))
        .expect("落盘");

    assert!(
        fixture
            .store
            .consume_continuation("continuation-1", "run:parent-session-1", CREATED_AT_MS + 10)
            .expect("首次消费"),
        "第一次消费就是那唯一一次有效执行"
    );
    // 并发或重复消费必须拿到 false 并放弃（合同 §8.2.2）。
    assert!(
        !fixture
            .store
            .consume_continuation("continuation-1", "tui:parent-session-1", CREATED_AT_MS + 11)
            .expect("重复消费")
    );

    let loaded = fixture
        .store
        .continuation("continuation-1")
        .expect("读取")
        .expect("存在");
    assert_eq!(loaded.state, AgentContinuationState::Consumed);
    assert_eq!(loaded.consumed_at_ms, Some(CREATED_AT_MS + 10));
    assert_eq!(
        loaded.consumed_by.as_deref(),
        Some("run:parent-session-1"),
        "消费方标识必须保留首次那一次，重复消费不得改写审计事实"
    );
}

#[test]
fn duplicate_after_consumption_reports_the_consumed_state() {
    let fixture = continuation_fixture();
    fixture
        .store
        .append_continuation(&continuation("continuation-1", "key-1"))
        .expect("落盘");
    fixture
        .store
        .consume_continuation("continuation-1", "run:parent-session-1", CREATED_AT_MS + 10)
        .expect("消费");

    assert_eq!(
        fixture
            .store
            .append_continuation(&continuation("continuation-2", "key-1"))
            .expect("重复落盘"),
        AgentContinuationAppendOutcome::Duplicate(AgentContinuationState::Consumed),
        "幂等命中必须回报既有记录的真实结局"
    );
}

#[test]
fn pending_scan_is_scoped_by_owner_and_parent_session() {
    let fixture = continuation_fixture();
    fixture
        .store
        .append_continuation(&continuation("continuation-1", "key-1"))
        .expect("落盘");
    // 同 owner 但不同父会话。
    let mut other_parent = continuation("continuation-2", "key-2");
    other_parent.parent_session_id = "parent-session-2".to_owned();
    fixture
        .store
        .append_continuation(&other_parent)
        .expect("落盘");
    // 同父会话但不同 owner 作用域。
    let mut other_owner = continuation("continuation-3", "key-3");
    other_owner.owner_id = "session-owner-2".to_owned();
    fixture
        .store
        .append_continuation(&other_owner)
        .expect("落盘");

    let pending = fixture
        .store
        .pending_continuations(OWNER_ID, PARENT_SESSION_ID, CREATED_AT_MS, 10)
        .expect("读取待装配");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].continuation_id, "continuation-1");
}

#[test]
fn pending_scan_skips_consumed_expired_and_backing_off_records() {
    let fixture = continuation_fixture();
    fixture
        .store
        .append_continuation(&continuation("continuation-consumed", "key-consumed"))
        .expect("落盘");
    fixture
        .store
        .consume_continuation("continuation-consumed", "run", CREATED_AT_MS + 1)
        .expect("消费");

    let mut expired = continuation("continuation-expired", "key-expired");
    expired.expires_at_ms = CREATED_AT_MS;
    fixture.store.append_continuation(&expired).expect("落盘");

    fixture
        .store
        .append_continuation(&continuation("continuation-backoff", "key-backoff"))
        .expect("落盘");
    fixture
        .store
        .record_continuation_failure(
            "continuation-backoff",
            "装配失败",
            CREATED_AT_MS + 2,
            CREATED_AT_MS + 5_000,
        )
        .expect("记账");

    fixture
        .store
        .append_continuation(&continuation("continuation-ready", "key-ready"))
        .expect("落盘");

    let pending = fixture
        .store
        .pending_continuations(OWNER_ID, PARENT_SESSION_ID, CREATED_AT_MS + 2, 10)
        .expect("读取待装配");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].continuation_id, "continuation-ready");

    // 退避到期后重新可见；已消费与已过期的始终不出现。
    let later = fixture
        .store
        .pending_continuations(OWNER_ID, PARENT_SESSION_ID, CREATED_AT_MS + 5_000, 10)
        .expect("读取待装配");
    assert_eq!(later.len(), 2);
    assert_eq!(later[0].continuation_id, "continuation-backoff");
    assert_eq!(later[1].continuation_id, "continuation-ready");
}

#[test]
fn failure_accounting_advances_attempts_without_dropping_the_fact() {
    let fixture = continuation_fixture();
    fixture
        .store
        .append_continuation(&continuation("continuation-1", "key-1"))
        .expect("落盘");

    for attempt in 1_u32..=3 {
        let now_ms = CREATED_AT_MS + u64::from(attempt);
        assert!(
            fixture
                .store
                .record_continuation_failure("continuation-1", "装配失败", now_ms, now_ms + 1_000,)
                .expect("记账")
        );
        let loaded = fixture
            .store
            .continuation("continuation-1")
            .expect("读取")
            .expect("存在");
        assert_eq!(loaded.attempt_count, attempt);
        assert_eq!(loaded.state, AgentContinuationState::Pending);
        assert_eq!(loaded.last_error.as_deref(), Some("装配失败"));
    }

    // 终态之后记账不生效，也不改写终态事实。
    fixture
        .store
        .consume_continuation("continuation-1", "run", CREATED_AT_MS + 100)
        .expect("消费");
    assert!(
        !fixture
            .store
            .record_continuation_failure(
                "continuation-1",
                "迟到错误",
                CREATED_AT_MS + 200,
                CREATED_AT_MS + 300,
            )
            .expect("记账")
    );
    let loaded = fixture
        .store
        .continuation("continuation-1")
        .expect("读取")
        .expect("存在");
    assert_eq!(loaded.state, AgentContinuationState::Consumed);
    assert_eq!(loaded.last_error, None);
}

#[test]
fn expiry_settles_only_records_past_retention_in_scope() {
    let fixture = continuation_fixture();
    let mut stale = continuation("continuation-stale", "key-stale");
    stale.expires_at_ms = CREATED_AT_MS + 100;
    fixture.store.append_continuation(&stale).expect("落盘");
    fixture
        .store
        .append_continuation(&continuation("continuation-live", "key-live"))
        .expect("落盘");

    // 别的会话不得被本次清扫波及。
    assert_eq!(
        fixture
            .store
            .expire_continuations("another-session", CREATED_AT_MS + 100, 16)
            .expect("结算"),
        0
    );
    assert_eq!(
        fixture
            .store
            .expire_continuations(OWNER_ID, CREATED_AT_MS + 100, 16)
            .expect("结算"),
        1
    );
    assert_eq!(
        fixture
            .store
            .continuation("continuation-stale")
            .expect("读取")
            .expect("存在")
            .state,
        AgentContinuationState::Expired
    );
    assert_eq!(
        fixture
            .store
            .continuation("continuation-live")
            .expect("读取")
            .expect("存在")
            .state,
        AgentContinuationState::Pending
    );
}

#[test]
fn query_filters_and_limit_apply() {
    let fixture = continuation_fixture();
    fixture
        .store
        .append_continuation(&continuation("continuation-1", "key-1"))
        .expect("落盘");
    fixture
        .store
        .append_continuation(&continuation("continuation-2", "key-2"))
        .expect("落盘");
    fixture
        .store
        .consume_continuation("continuation-2", "run", CREATED_AT_MS + 5)
        .expect("消费");

    let all = fixture
        .store
        .list_continuations(&AgentContinuationQuery {
            owner_id: Some(OWNER_ID.to_owned()),
            limit: 10,
            ..AgentContinuationQuery::default()
        })
        .expect("查询");
    assert_eq!(all.len(), 2);

    let pending = fixture
        .store
        .list_continuations(&AgentContinuationQuery {
            child_session_id: Some(CHILD_SESSION_ID.to_owned()),
            state: Some(AgentContinuationState::Pending),
            limit: 10,
            ..AgentContinuationQuery::default()
        })
        .expect("查询");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].continuation_id, "continuation-1");

    assert!(
        fixture
            .store
            .list_continuations(&AgentContinuationQuery {
                limit: 0,
                ..AgentContinuationQuery::default()
            })
            .expect("查询")
            .is_empty(),
        "limit=0 必须早退而不是全量返回"
    );
}

#[test]
fn continuation_reference_is_resolvable_after_reopen() {
    // 合同硬性约束：引用必须重启后可解析：artifact 用磁盘路径、摘要用落盘文本，
    // 且 job_ref 不参与解析。
    let fixture = continuation_fixture();
    fixture
        .store
        .append_continuation(&continuation("continuation-1", "key-1"))
        .expect("落盘");
    let database_path = fixture.database_path.clone();
    drop(fixture.store);

    let reopened = SqliteRuntimeStore::new(&database_path);
    let pending = reopened
        .pending_continuations(OWNER_ID, PARENT_SESSION_ID, CREATED_AT_MS, 10)
        .expect("读取待装配");
    assert_eq!(pending.len(), 1);
    let record = &pending[0];
    assert_eq!(
        record.summary, "审查完成：发现 2 处命名问题",
        "摘要是落盘文本，重启后原样可读"
    );
    assert_eq!(
        record.artifact_path.as_deref(),
        Some(ARTIFACT_PATH),
        "artifact 是磁盘绝对路径，重启后仍是可解析凭据"
    );
    assert_eq!(
        record.job_ref.as_deref(),
        Some("epoch-1:7"),
        "job_ref 只作可读线索保留，不承担解析职责"
    );
    assert_eq!(record.token_output, Some(45));
}

#[test]
fn legacy_database_without_continuation_table_gains_it_on_open() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    {
        // 手造「新增本表之前」的既有库：只有 schema identity。
        let connection = rusqlite::Connection::open(&path).expect("打开数据库");
        connection
            .execute_batch(
                "CREATE TABLE jarvis_schema_identity (identity TEXT PRIMARY KEY NOT NULL);
                 INSERT INTO jarvis_schema_identity(identity) VALUES('jarvis-runtime-goal-v1');",
            )
            .expect("造旧库");
    }
    let store = SqliteRuntimeStore::new(&path);
    store
        .append_continuation(&continuation("continuation-1", "key-1"))
        .expect("既有库必须能直接写入新表");
    assert_eq!(count_rows(&store, "key-1"), 1);
}
