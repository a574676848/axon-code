//! `jarvis_agent_messages` 的落盘、幂等与投递状态机测试（合同 §5.3.1 / §5.3.2）。

use super::SqliteRuntimeStore;
use crate::{
    AgentMailboxAppendOutcome, AgentMailboxMessageKind, AgentMailboxMessageRecord,
    AgentMailboxMessageState, AgentMailboxQuery, AgentMailboxRetryPolicy, AgentMailboxStore,
};

const OWNER_ID: &str = "session-owner-1";
const CREATED_AT_MS: u64 = 1_000;
const RETENTION_MS: u64 = 86_400_000;
const BACKOFF_MS: u64 = 1_000;
const MAX_BACKOFF_MS: u64 = 8_000;
const MAX_ATTEMPTS: u32 = 3;

fn retry_policy() -> AgentMailboxRetryPolicy {
    AgentMailboxRetryPolicy {
        backoff_ms: BACKOFF_MS,
        max_backoff_ms: MAX_BACKOFF_MS,
        max_attempts: MAX_ATTEMPTS,
    }
}

struct MailboxFixture {
    store: SqliteRuntimeStore,
    // 字段顺序即析构顺序：临时目录必须最后析构，`store` 先释放 SQLite 句柄，目录才删得掉。
    _directory: axon_test_support::TestTempDir,
}

fn mailbox_fixture() -> MailboxFixture {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    MailboxFixture {
        store,
        _directory: directory,
    }
}

fn message(message_id: &str, idempotency_key: &str) -> AgentMailboxMessageRecord {
    AgentMailboxMessageRecord::pending(
        message_id,
        idempotency_key,
        OWNER_ID,
        "/root",
        "/root/review",
        AgentMailboxMessageKind::Collab,
        "继续处理评审项",
        false,
        CREATED_AT_MS,
        CREATED_AT_MS + RETENTION_MS,
    )
}

/// 直接打开底层库断言物理行数，避免只验证内存视图。
fn count_rows(store: &SqliteRuntimeStore, sql: &str, key: &str) -> i64 {
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    connection
        .query_row(sql, rusqlite::params![key], |row| row.get(0))
        .expect("读取行数")
}

#[test]
fn message_round_trips_all_fields() {
    let fixture = mailbox_fixture();
    let record = message("msg-1", "collab:owner:root:review:1000");
    assert_eq!(
        fixture.store.append_agent_message(&record).expect("落盘"),
        AgentMailboxAppendOutcome::Inserted
    );

    let loaded = fixture
        .store
        .agent_message("msg-1")
        .expect("读取")
        .expect("存在");
    assert_eq!(loaded, record);
    assert_eq!(loaded.state, AgentMailboxMessageState::Pending);
    assert_eq!(loaded.attempt_count, 0);
    assert!(loaded.state.is_deliverable());

    let by_key = fixture
        .store
        .agent_message_by_idempotency_key("collab:owner:root:review:1000")
        .expect("读取")
        .expect("存在");
    assert_eq!(by_key.message_id, "msg-1");
}

#[test]
fn duplicate_idempotency_key_writes_exactly_one_message() {
    let fixture = mailbox_fixture();
    let first = message("msg-1", "collab:owner:root:review:1000");
    assert_eq!(
        fixture
            .store
            .append_agent_message(&first)
            .expect("首次落盘"),
        AgentMailboxAppendOutcome::Inserted
    );

    // 同键不同 message_id：幂等键已经绑定首条消息，第二条不得落盘，
    // 且必须回报既有记录的结局而不是简单的「失败」。
    let duplicate = message("msg-2", "collab:owner:root:review:1000");
    assert_eq!(
        fixture
            .store
            .append_agent_message(&duplicate)
            .expect("重复落盘"),
        AgentMailboxAppendOutcome::Duplicate(AgentMailboxMessageState::Pending)
    );

    assert_eq!(
        count_rows(
            &fixture.store,
            "SELECT COUNT(*) FROM jarvis_agent_messages WHERE idempotency_key=?1",
            "collab:owner:root:review:1000",
        ),
        1
    );
    assert!(
        fixture
            .store
            .agent_message("msg-2")
            .expect("读取")
            .is_none()
    );
}

#[test]
fn duplicate_reports_existing_delivery_state() {
    let fixture = mailbox_fixture();
    fixture
        .store
        .append_agent_message(&message("msg-1", "key-1"))
        .expect("落盘");
    fixture
        .store
        .deliver_agent_message("msg-1", CREATED_AT_MS + 10)
        .expect("投递");

    assert_eq!(
        fixture
            .store
            .append_agent_message(&message("msg-2", "key-1"))
            .expect("重复落盘"),
        AgentMailboxAppendOutcome::Duplicate(AgentMailboxMessageState::Delivered),
        "幂等命中必须回报既有记录的真实结局，调用方才能如实回执"
    );
}

#[test]
fn pending_messages_are_scoped_by_owner_and_recipient() {
    let fixture = mailbox_fixture();
    fixture
        .store
        .append_agent_message(&message("msg-1", "key-1"))
        .expect("落盘");
    // 同 ID 但不同 owner 作用域：同名 AgentPath 必须被隔离。
    let other_owner = AgentMailboxMessageRecord::pending(
        "msg-2",
        "key-2",
        "session-owner-2",
        "/root",
        "/root/review",
        AgentMailboxMessageKind::Collab,
        "另一个会话的消息",
        false,
        CREATED_AT_MS,
        CREATED_AT_MS + RETENTION_MS,
    );
    fixture
        .store
        .append_agent_message(&other_owner)
        .expect("落盘");
    // 同 owner 但不同接收方。
    let other_recipient = AgentMailboxMessageRecord::pending(
        "msg-3",
        "key-3",
        OWNER_ID,
        "/root",
        "/root/other",
        AgentMailboxMessageKind::Collab,
        "别的子代理的消息",
        false,
        CREATED_AT_MS,
        CREATED_AT_MS + RETENTION_MS,
    );
    fixture
        .store
        .append_agent_message(&other_recipient)
        .expect("落盘");

    let pending = fixture
        .store
        .pending_agent_messages(OWNER_ID, "/root/review", CREATED_AT_MS, 10)
        .expect("读取待投递");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message_id, "msg-1");
}

#[test]
fn pending_messages_skip_expired_and_not_yet_due_records() {
    let fixture = mailbox_fixture();
    let mut not_due = message("msg-due", "key-due");
    not_due.retry_at_ms = Some(CREATED_AT_MS + 5_000);
    fixture.store.append_agent_message(&not_due).expect("落盘");
    let mut expired = message("msg-expired", "key-expired");
    expired.expires_at_ms = CREATED_AT_MS;
    fixture.store.append_agent_message(&expired).expect("落盘");
    fixture
        .store
        .append_agent_message(&message("msg-ready", "key-ready"))
        .expect("落盘");

    let pending = fixture
        .store
        .pending_agent_messages(OWNER_ID, "/root/review", CREATED_AT_MS, 10)
        .expect("读取待投递");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message_id, "msg-ready");

    // 到期后未到重试时刻的那条才重新可见；已就绪的那条始终在集合中。
    let later = fixture
        .store
        .pending_agent_messages(OWNER_ID, "/root/review", CREATED_AT_MS + 5_000, 10)
        .expect("读取待投递");
    assert_eq!(later.len(), 2);
    assert_eq!(later[0].message_id, "msg-ready");
    assert_eq!(later[1].message_id, "msg-due");
}

#[test]
fn delivery_transition_happens_at_most_once() {
    let fixture = mailbox_fixture();
    fixture
        .store
        .append_agent_message(&message("msg-1", "key-1"))
        .expect("落盘");

    assert!(
        fixture
            .store
            .deliver_agent_message("msg-1", CREATED_AT_MS + 10)
            .expect("投递")
    );
    // 同一消息的第二次迁移不生效：状态机不允许 delivered 回退或重复迁移。
    assert!(
        !fixture
            .store
            .deliver_agent_message("msg-1", CREATED_AT_MS + 20)
            .expect("重复投递")
    );

    let loaded = fixture
        .store
        .agent_message("msg-1")
        .expect("读取")
        .expect("存在");
    assert_eq!(loaded.state, AgentMailboxMessageState::Delivered);
    assert_eq!(loaded.delivered_at_ms, Some(CREATED_AT_MS + 10));
    // Delivered 不是终态：它只说消息进了目标 mailbox，读确认才结束这条消息的
    // 生命周期；把它当终态，重启后就再也找不回「已送达但没被读过」的消息
    // （合同 §8.2.5）。
    assert!(!loaded.state.is_terminal());
    assert!(!loaded.state.is_deliverable());
}

#[test]
fn failure_accounting_backs_off_then_expires_at_attempt_limit() {
    let fixture = mailbox_fixture();
    fixture
        .store
        .append_agent_message(&message("msg-1", "key-1"))
        .expect("落盘");

    let policy = retry_policy();
    let mut now = CREATED_AT_MS;
    for attempt in 1..MAX_ATTEMPTS {
        let expected_retry_at = now + BACKOFF_MS * (1_u64 << (attempt - 1));
        let exhausted = fixture
            .store
            .record_agent_message_failure("msg-1", "目标不可用", now, policy)
            .expect("记账");
        assert!(!exhausted, "第 {attempt} 次失败不应终止重试");
        let loaded = fixture
            .store
            .agent_message("msg-1")
            .expect("读取")
            .expect("存在");
        assert_eq!(loaded.attempt_count, attempt);
        assert_eq!(loaded.state, AgentMailboxMessageState::Pending);
        assert_eq!(
            loaded.retry_at_ms,
            Some(expected_retry_at),
            "退避必须按尝试次数指数增长"
        );
        assert_eq!(loaded.last_error.as_deref(), Some("目标不可用"));
        // 退避期间不出现在待投递集合中。
        assert!(
            fixture
                .store
                .pending_agent_messages(OWNER_ID, "/root/review", now, 10)
                .expect("读取待投递")
                .is_empty()
        );
        now = expected_retry_at;
    }

    let exhausted = fixture
        .store
        .record_agent_message_failure("msg-1", "目标不可用", now, policy)
        .expect("记账");
    assert!(exhausted, "达到上限的那次必须回报终止重试");
    let loaded = fixture
        .store
        .agent_message("msg-1")
        .expect("读取")
        .expect("存在");
    assert_eq!(loaded.attempt_count, MAX_ATTEMPTS);
    assert_eq!(loaded.state, AgentMailboxMessageState::Expired);
    assert_eq!(loaded.retry_at_ms, None);
}

#[test]
fn retry_policy_caps_the_backoff_window() {
    let policy = retry_policy();
    assert_eq!(policy.retry_at_ms(1_000, 1), 1_000 + BACKOFF_MS);
    assert_eq!(policy.retry_at_ms(1_000, 2), 1_000 + BACKOFF_MS * 2);
    assert_eq!(policy.retry_at_ms(1_000, 3), 1_000 + BACKOFF_MS * 4);
    // 继续翻倍会越过上限，必须被压到 max_backoff_ms。
    assert_eq!(policy.retry_at_ms(1_000, 4), 1_000 + MAX_BACKOFF_MS);
    assert_eq!(policy.retry_at_ms(1_000, 64), 1_000 + MAX_BACKOFF_MS);
}

#[test]
fn failure_accounting_is_noop_after_terminal_state() {
    let fixture = mailbox_fixture();
    fixture
        .store
        .append_agent_message(&message("msg-1", "key-1"))
        .expect("落盘");
    assert!(
        fixture
            .store
            .deliver_agent_message("msg-1", CREATED_AT_MS + 10)
            .expect("投递")
    );

    // 迟到失败记账不得改写已投递消息，也不要求调用方继续处理。
    assert!(
        fixture
            .store
            .record_agent_message_failure("msg-1", "迟到错误", CREATED_AT_MS + 20, retry_policy())
            .expect("记账")
    );
    let loaded = fixture
        .store
        .agent_message("msg-1")
        .expect("读取")
        .expect("存在");
    assert_eq!(loaded.state, AgentMailboxMessageState::Delivered);
    assert_eq!(loaded.attempt_count, 0);
    assert_eq!(loaded.last_error, None);
}

#[test]
fn expiry_settles_only_records_past_retention() {
    let fixture = mailbox_fixture();
    let mut stale = message("msg-stale", "key-stale");
    stale.expires_at_ms = CREATED_AT_MS + 100;
    fixture.store.append_agent_message(&stale).expect("落盘");
    fixture
        .store
        .append_agent_message(&message("msg-live", "key-live"))
        .expect("落盘");

    let settled = fixture
        .store
        .expire_agent_messages(OWNER_ID, CREATED_AT_MS + 100, 16)
        .expect("结算");
    assert_eq!(settled, 1);
    // 过期结算同样按作用域隔离：别的会话不得被本次清扫波及。
    assert_eq!(
        fixture
            .store
            .expire_agent_messages("another-session", CREATED_AT_MS + 100, 16)
            .expect("结算"),
        0
    );
    assert_eq!(
        fixture
            .store
            .agent_message("msg-stale")
            .expect("读取")
            .expect("存在")
            .state,
        AgentMailboxMessageState::Expired
    );
    assert_eq!(
        fixture
            .store
            .agent_message("msg-live")
            .expect("读取")
            .expect("存在")
            .state,
        AgentMailboxMessageState::Pending
    );
}

#[test]
fn due_scan_returns_only_retryable_records_in_order() {
    let fixture = mailbox_fixture();
    let mut second = message("msg-second", "key-second");
    second.retry_at_ms = Some(CREATED_AT_MS + 20);
    let mut first = message("msg-first", "key-first");
    first.retry_at_ms = Some(CREATED_AT_MS + 10);
    fixture.store.append_agent_message(&second).expect("落盘");
    fixture.store.append_agent_message(&first).expect("落盘");
    fixture
        .store
        .append_agent_message(&message("msg-immediate", "key-immediate"))
        .expect("落盘");

    let due = fixture
        .store
        .due_agent_messages(OWNER_ID, CREATED_AT_MS + 10, 2)
        .expect("扫描");
    // 未到时刻的 msg-second 不出现；有界条数生效。
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].message_id, "msg-immediate");
    assert_eq!(due[1].message_id, "msg-first");

    // 重试按作用域过滤：别的会话的消息不得被拉进本次装配。
    assert!(
        fixture
            .store
            .due_agent_messages("another-session", CREATED_AT_MS + 100, 10)
            .expect("扫描")
            .is_empty()
    );
}

#[test]
fn query_filters_and_limit_apply() {
    let fixture = mailbox_fixture();
    fixture
        .store
        .append_agent_message(&message("msg-1", "key-1"))
        .expect("落盘");
    let completion = AgentMailboxMessageRecord::pending(
        "msg-2",
        "key-2",
        OWNER_ID,
        "/root/review",
        "/root",
        AgentMailboxMessageKind::Completion,
        "子代理已完成",
        false,
        CREATED_AT_MS + 1,
        CREATED_AT_MS + RETENTION_MS,
    )
    .with_continuation("continuation-1");
    fixture
        .store
        .append_agent_message(&completion)
        .expect("落盘");
    fixture
        .store
        .deliver_agent_message("msg-2", CREATED_AT_MS + 2)
        .expect("投递");

    let all = fixture
        .store
        .list_agent_messages(&AgentMailboxQuery {
            owner_id: Some(OWNER_ID.to_owned()),
            limit: 10,
            ..AgentMailboxQuery::default()
        })
        .expect("查询");
    assert_eq!(all.len(), 2);

    let delivered = fixture
        .store
        .list_agent_messages(&AgentMailboxQuery {
            owner_id: Some(OWNER_ID.to_owned()),
            state: Some(AgentMailboxMessageState::Delivered),
            limit: 10,
            ..AgentMailboxQuery::default()
        })
        .expect("查询");
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].message_id, "msg-2");
    assert_eq!(
        delivered[0].kind,
        AgentMailboxMessageKind::Completion,
        "完成消息的类别与 continuation 引用必须往返保真"
    );
    assert_eq!(
        delivered[0].continuation_id.as_deref(),
        Some("continuation-1")
    );

    let limited = fixture
        .store
        .list_agent_messages(&AgentMailboxQuery {
            owner_id: Some(OWNER_ID.to_owned()),
            limit: 1,
            ..AgentMailboxQuery::default()
        })
        .expect("查询");
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].message_id, "msg-1");

    let empty = fixture
        .store
        .list_agent_messages(&AgentMailboxQuery {
            owner_id: Some(OWNER_ID.to_owned()),
            limit: 0,
            ..AgentMailboxQuery::default()
        })
        .expect("查询");
    assert!(empty.is_empty(), "limit=0 必须早退而不是全量返回");
}

#[test]
fn mailbox_survives_reopen() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    {
        let store = SqliteRuntimeStore::new(&path);
        store
            .append_agent_message(&message("msg-1", "key-1"))
            .expect("落盘");
    }
    // 新进程语义：重新打开同一个库，未投递消息仍在。
    let reopened = SqliteRuntimeStore::new(&path);
    let pending = reopened
        .pending_agent_messages(OWNER_ID, "/root/review", CREATED_AT_MS, 10)
        .expect("读取待投递");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "继续处理评审项");
}

#[test]
fn legacy_database_without_mailbox_table_gains_it_on_open() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    {
        // 手造「新增本表之前」的既有库：只有 schema identity，没有任何业务表。
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
        .append_agent_message(&message("msg-1", "key-1"))
        .expect("既有库必须能直接写入新表");
    assert_eq!(
        count_rows(
            &store,
            "SELECT COUNT(*) FROM jarvis_agent_messages WHERE message_id=?1",
            "msg-1",
        ),
        1
    );
}

/// 读确认与投递确认分离：只有已投递的记录才能推进到已读（合同 §8.2.5）。
#[test]
fn read_confirmation_requires_a_prior_delivery() {
    let fixture = mailbox_fixture();
    let store = &fixture.store;
    store
        .append_agent_message(&message("m-read", "key-read"))
        .expect("写入");

    assert!(
        !store.read_agent_message("m-read", 1_100).expect("读确认"),
        "未投递的记录不得被标记已读：读确认以前置的投递确认为前提"
    );
    assert!(store.deliver_agent_message("m-read", 1_100).expect("投递"));
    assert!(store.read_agent_message("m-read", 1_200).expect("读确认"));
    assert!(
        !store
            .read_agent_message("m-read", 1_300)
            .expect("重复读确认"),
        "读确认只生效一次"
    );

    let record = store.agent_message("m-read").expect("读取").expect("存在");
    assert_eq!(record.state, AgentMailboxMessageState::Read);
    assert!(record.state.is_terminal(), "已读是终态");
    assert!(!record.state.is_deliverable(), "已读不再参与投递");
    assert_eq!(AgentMailboxMessageState::parse("read"), Some(record.state));
}

/// 「已投递未读」查询：只返回未读的已投递记录，按投递顺序、有界且按作用域隔离。
#[test]
fn delivered_unread_query_returns_only_unread_delivered_records() {
    let fixture = mailbox_fixture();
    let store = &fixture.store;
    for (message_id, key) in [("m-1", "key-1"), ("m-2", "key-2"), ("m-3", "key-3")] {
        store
            .append_agent_message(&message(message_id, key))
            .expect("写入");
    }

    assert!(
        store
            .delivered_unread_agent_messages(OWNER_ID, "/root/review", 1_100, 8)
            .expect("查询")
            .is_empty(),
        "未投递的记录不属于「已投递未读」"
    );
    store.deliver_agent_message("m-1", 1_100).expect("投递");
    store.deliver_agent_message("m-2", 1_110).expect("投递");
    store.deliver_agent_message("m-3", 1_120).expect("投递");
    store.read_agent_message("m-2", 1_130).expect("读确认");

    let unread = store
        .delivered_unread_agent_messages(OWNER_ID, "/root/review", 1_200, 8)
        .expect("查询");
    assert_eq!(
        unread
            .iter()
            .map(|record| record.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["m-1", "m-3"],
        "只返回未读的已投递记录，按投递顺序"
    );
    assert_eq!(
        store
            .delivered_unread_agent_messages(OWNER_ID, "/root/review", 1_200, 1)
            .expect("有界查询")
            .len(),
        1,
        "limit 必须生效"
    );
    assert!(
        store
            .delivered_unread_agent_messages("other-owner", "/root/review", 1_200, 8)
            .expect("跨作用域")
            .is_empty(),
        "查询按 owner 作用域隔离"
    );
}
