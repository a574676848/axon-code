//! `jarvis_user_questions` 的持久化、幂等、CAS 结算与过期回收测试。

use super::sqlite_runtime_store::SqliteRuntimeStore;
use crate::ports::{UserQuestionStore, UserQuestionStoreError};
use axon_domain::{UserQuestionRecord, UserQuestionState};

struct Fixture {
    store: SqliteRuntimeStore,
    _directory: axon_test_support::TestTempDir,
}

fn fixture() -> Fixture {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    Fixture {
        store,
        _directory: directory,
    }
}

fn test_record(qid: &str, idempotency_key: &str, tool_call_id: &str) -> UserQuestionRecord {
    UserQuestionRecord {
        question_id: qid.to_string(),
        idempotency_key: idempotency_key.to_string(),
        lane_id: "lane-test".to_string(),
        session_id: "sess-1".to_string(),
        interaction_id: "inter-1".to_string(),
        turn_id: "turn-1".to_string(),
        tool_call_id: tool_call_id.to_string(),
        questions_json: r#"[{"id":"q1","question":"是否继续？"}]"#.to_string(),
        handoff_text: None,
        state: UserQuestionState::Pending,
        deadline_at_ms: 30_000,
        answer_json: None,
        answered_at_ms: None,
        answered_channel: None,
        delivered: false,
        delivery_mode: None,
        created_at_ms: 1_000,
        expires_at_ms: 86_400_000,
    }
}

#[test]
fn test_insert_and_idempotency() {
    let fix = fixture();
    let rec1 = test_record("uq-1", "idem-1", "call-1");

    // 首次插入成功
    let inserted = fix.store.insert_question(&rec1).expect("首次插入应成功");
    assert_eq!(inserted.question_id, "uq-1");

    // 相同 idempotency_key 再次插入应幂等返回既有记录
    let mut rec2 = test_record("uq-different", "idem-1", "call-2");
    rec2.lane_id = "lane-other".to_string();
    let second = fix.store.insert_question(&rec2).expect("幂等插入应成功");
    assert_eq!(second.question_id, "uq-1");
    assert_eq!(second.idempotency_key, "idem-1");

    // 物理检查只存在 1 行
    let conn = rusqlite::Connection::open(fix.store.path()).expect("打开库");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM jarvis_user_questions WHERE idempotency_key = 'idem-1'",
            [],
            |r| r.get(0),
        )
        .expect("查询行数");
    assert_eq!(count, 1);
}

#[test]
fn test_cas_settle_answer_success_and_conflict() {
    let fix = fixture();
    let rec = test_record("uq-2", "idem-2", "call-2");
    fix.store.insert_question(&rec).expect("插入应成功");

    // 首次结算成功
    let settled = fix
        .store
        .cas_settle_answer(
            "call-2",
            r#"[{"id":"q1","selected":["继续"]}]"#,
            35_000,
            Some("desktop"),
            Some("tool_return"),
        )
        .expect("首次结算应成功");

    assert_eq!(settled.state, UserQuestionState::Answered);
    assert_eq!(settled.answered_channel.as_deref(), Some("desktop"));
    assert_eq!(settled.delivery_mode.as_deref(), Some("tool_return"));
    assert!(!settled.delivered);

    // 二次结算应被拒绝（AlreadySettled）
    let err = fix
        .store
        .cas_settle_answer(
            "call-2",
            r#"[{"id":"q1","selected":["取消"]}]"#,
            40_000,
            Some("tui"),
            Some("user_message"),
        )
        .expect_err("已结算问题不应再次结算");

    match err {
        UserQuestionStoreError::AlreadySettled(_) => {}
        other => panic!("期望 AlreadySettled 错误，收到 {other:?}"),
    }

    // 未知 call_id 结算应返回 NotFound
    let not_found_err = fix
        .store
        .cas_settle_answer("unknown-call", "{}", 50_000, None, None)
        .expect_err("未知 call 应返回 NotFound");
    match not_found_err {
        UserQuestionStoreError::NotFound(_) => {}
        other => panic!("期望 NotFound 错误，收到 {other:?}"),
    }
}

#[test]
fn test_cas_update_state_and_deadline() {
    let fix = fixture();
    let rec = test_record("uq-3", "idem-3", "call-3");
    fix.store.insert_question(&rec).expect("插入应成功");

    // Pending -> Suspended
    let ok = fix
        .store
        .cas_update_state(
            "uq-3",
            UserQuestionState::Pending,
            UserQuestionState::Suspended,
            None,
            Some("超时已挂起"),
        )
        .expect("CAS 挂起应成功");
    assert!(ok);

    let fetched = fix
        .store
        .get_question_by_id("uq-3")
        .expect("查询应成功")
        .expect("应存在");
    assert_eq!(fetched.state, UserQuestionState::Suspended);
    assert_eq!(fetched.handoff_text.as_deref(), Some("超时已挂起"));

    // Suspended 状态下再次尝试以 Pending 为原状态更新应返回 false
    let failed = fix
        .store
        .cas_update_state(
            "uq-3",
            UserQuestionState::Pending,
            UserQuestionState::Cancelled,
            None,
            None,
        )
        .expect("CAS 操作");
    assert!(!failed);
}

#[test]
fn test_reconciliation_and_mark_delivered() {
    let fix = fixture();
    let rec = test_record("uq-4", "idem-4", "call-4");
    fix.store.insert_question(&rec).expect("插入应成功");

    fix.store
        .cas_settle_answer(
            "call-4",
            r#"[{"id":"q1","selected":["好的"]}]"#,
            2_000,
            Some("feishu"),
            Some("user_message"),
        )
        .expect("结算应成功");

    // 扫描未投递行
    let list = fix
        .store
        .list_unreconciled_answered(10)
        .expect("扫描对账应成功");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].question_id, "uq-4");
    assert!(!list[0].delivered);

    // 标记投递完成并落库投递方式
    let marked = fix
        .store
        .mark_delivered("uq-4", "user_message")
        .expect("标记投递应成功");
    assert!(marked);

    let delivered = fix
        .store
        .get_question_by_id("uq-4")
        .expect("查询应成功")
        .expect("应存在");
    assert!(delivered.delivered);
    assert_eq!(delivered.delivery_mode.as_deref(), Some("user_message"));

    // 再次扫描应为空
    let list_after = fix
        .store
        .list_unreconciled_answered(10)
        .expect("扫描对账应成功");
    assert_eq!(list_after.len(), 0);
}

#[test]
fn test_mark_expired_questions_keeps_auditable_state() {
    let fix = fixture();
    let mut rec1 = test_record("uq-5", "idem-5", "call-5");
    rec1.expires_at_ms = 10_000;
    fix.store.insert_question(&rec1).expect("插入 1");

    let mut rec2 = test_record("uq-6", "idem-6", "call-6");
    rec2.expires_at_ms = 50_000;
    fix.store.insert_question(&rec2).expect("插入 2");

    // 已作答的记录不因过期被改写终态。
    let mut answered = test_record("uq-7", "idem-7", "call-7");
    answered.expires_at_ms = 10_000;
    fix.store.insert_question(&answered).expect("插入 3");
    fix.store
        .cas_settle_answer(
            "call-7",
            r#"[{"id":"q1","selected":["是"]}]"#,
            2_000,
            None,
            None,
        )
        .expect("结算应成功");

    // 当前时间 20_000：uq-5 越过自身过期时刻被标记，uq-6 未到期保持 pending。
    let marked = fix
        .store
        .mark_expired_questions(20_000, 100)
        .expect("过期标记应成功");
    assert_eq!(marked, 1);

    let expired = fix
        .store
        .get_question_by_id("uq-5")
        .expect("查询 1")
        .expect("记录必须保留下来供审计");
    assert_eq!(expired.state, UserQuestionState::Expired);
    assert_eq!(
        fix.store
            .get_question_by_id("uq-6")
            .expect("查询 2")
            .expect("应存在")
            .state,
        UserQuestionState::Pending
    );
    assert_eq!(
        fix.store
            .get_question_by_id("uq-7")
            .expect("查询 3")
            .expect("应存在")
            .state,
        UserQuestionState::Answered
    );
}
