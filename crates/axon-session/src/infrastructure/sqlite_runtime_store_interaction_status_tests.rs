//! 车道历史页的状态派生：历史行带出来的队列终态词汇必须回读成 Interaction 状态。
//!
//! 取消与失败路径只写 `work_items.status`，终态行搬进历史表时 `terminal_kind` 取
//! `COALESCE(terminal_kind, status)`，于是历史里留下的是队列词汇。读侧漏认这些词，
//! 含该轮的整条车道历史页会整体读不出来。

use super::SqliteRuntimeStore;
use super::sqlite_runtime_store_interaction_fixtures::{interaction_record, test_session};
use crate::ports::{InteractionQueryStore as _, InteractionStore as _, RuntimeStore as _};
use axon_domain::InteractionStatus;

/// 历史里可能出现的终态词汇，与它应当还原成的 Interaction 状态。
const HISTORICAL_TERMINAL_KINDS: [(&str, InteractionStatus); 4] = [
    ("understood", InteractionStatus::Understood),
    ("completed", InteractionStatus::Understood),
    ("cancelled", InteractionStatus::Interrupted),
    ("failed", InteractionStatus::Interrupted),
];

#[test]
fn lane_history_reads_historical_terminal_kinds_as_interaction_status() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session = test_session("session-terminal-kinds");
    store.initialize().expect("初始化 schema");
    write_inbox_rows(
        &store,
        &session,
        HISTORICAL_TERMINAL_KINDS.iter().map(|(kind, _)| *kind),
    );

    let records = store
        .interactions_for_session(session.as_str(), 10, None)
        .expect("车道历史必须可读");
    let statuses: Vec<InteractionStatus> = records.iter().map(|record| record.status).collect();
    let expected: Vec<InteractionStatus> = HISTORICAL_TERMINAL_KINDS
        .iter()
        .map(|(_, status)| *status)
        .collect();
    assert_eq!(statuses, expected);
}

#[test]
fn unrecognized_terminal_kind_reports_the_value_instead_of_a_sql_error() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session = test_session("session-unknown-kind");
    store.initialize().expect("初始化 schema");
    write_inbox_rows(&store, &session, std::iter::once("quantum"));

    let error = store
        .interactions_for_session(session.as_str(), 10, None)
        .expect_err("陌生词汇必须显式报错");
    // 对外只给产品语义：用户不需要认识内部状态词表。
    let message = error.to_string();
    assert!(
        message.contains("无法识别的状态记录"),
        "错误信息必须能被人读懂：{message}"
    );
    assert!(
        !message.contains("read-only"),
        "不得再回 rusqlite 的误导文案：{message}"
    );
    // 原始取值只进日志（`unknown_status` 里落盘），不再出现在用户可见文案里。
    assert!(
        !message.contains("quantum"),
        "内部词表不得混进用户可见文案：{message}"
    );
}

/// 写入入站事实与终态历史行：取消与失败路径两个列（`status` 与 `terminal_kind`）同词。
fn write_inbox_rows<'a>(
    store: &SqliteRuntimeStore,
    session: &axon_domain::DaemonSessionId,
    terminal_kinds: impl Iterator<Item = &'a str>,
) {
    let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
    for (index, terminal_kind) in terminal_kinds.enumerate() {
        let received_at_ms = index as u64 * 10;
        let interaction_id = format!("interaction-{terminal_kind}");
        let interaction = interaction_record(
            session,
            &interaction_id,
            &format!("message-{terminal_kind}"),
            received_at_ms,
        );
        store.append(&interaction).expect("写入入站事实");
        connection
            .execute(
                "INSERT INTO work_item_history(work_id,kind,attempt,priority,status,terminal_kind,\
                 inbox_id,created_at_ms,updated_at_ms,finished_at_ms) \
                 VALUES(?1,'turn',1,0,?2,?3,?4,?5,?5,?5)",
                rusqlite::params![
                    format!("work-{terminal_kind}"),
                    terminal_kind,
                    terminal_kind,
                    interaction_id,
                    received_at_ms as i64
                ],
            )
            .expect("写入终态历史行");
    }
}
