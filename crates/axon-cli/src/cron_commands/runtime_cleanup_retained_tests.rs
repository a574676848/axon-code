//! 事件保留期保底语义的回归用例。

use super::tests::open_fixture;
use super::*;

/// 已终结会话的超期事件必须整段回收，不受活跃车道保底条数限制。
///
/// 旧谓词对所有会话无条件保留最新一段事件，于是每个死会话的最后若干条
/// 永久驻留，7 天保留期对这类会话完全失效。
#[test]
fn finished_session_events_are_reclaimed_below_active_floor() {
    const EVENT_COUNT: i64 = 3;
    const FINISHED_SESSION_ID: &str = "s-done";
    let expected = usize::try_from(EVENT_COUNT).expect("计数应可转换");
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute(
            "INSERT INTO session_runtimes
             VALUES(?1,?1,?1,'/w','completed',NULL,NULL,0,0,0,?2,1,1,1)",
            rusqlite::params![FINISHED_SESSION_ID, EVENT_COUNT],
        )
        .expect("终态 Session 应可写入");
    connection
        .execute(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < ?2)
             INSERT INTO session_events(
                 session_id, sequence, generation, event_type, payload_json, emitted_at_ms
             )
             SELECT ?1, n, 'g', 'turn', '{}', 1 FROM seq",
            rusqlite::params![FINISHED_SESSION_ID, EVENT_COUNT],
        )
        .expect("终态 Session 事件应可写入");
    drop(connection);

    let dry_run = count_runtime_cleanup(root.path(), 100).expect("dry-run 应成功");
    assert_eq!(dry_run.session_events, expected);

    let counts = cleanup_runtime_data(root.path(), 100).expect("清理应成功");
    assert_eq!(counts.session_events, expected);

    let connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(RUNTIME_DATABASE_FILE),
    )
    .expect("测试数据库应可重开");
    let remaining: i64 = connection
        .query_row("SELECT COUNT(*) FROM session_events", [], |row| row.get(0))
        .expect("统计应成功");
    assert_eq!(remaining, 0, "终态 Session 的超期事件必须全部回收");
}
