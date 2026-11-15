//! 外置正文生命周期与悬空引用的清理回归。

use super::tests::open_fixture;
use super::*;

#[test]
fn session_family_cleanup_reclaims_external_payload_directory() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute(
            "INSERT INTO session_runtimes VALUES('s-payload','s-payload','s-payload','/w','completed',NULL,NULL,0,0,0,0,1,1,1)",
            [],
        )
        .expect("Session 应可写入");
    drop(connection);

    let archive_root = root
        .path()
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join("archive")
        .join("session-events");
    let payload_path = archive_root.join("s-payload").join("7.json");
    std::fs::create_dir_all(payload_path.parent().expect("父目录")).expect("目录应可创建");
    std::fs::write(&payload_path, b"{\"legacy\":true}").expect("外置正文应可写入");
    let offline = archive_root.join("s-payload.jsonl.zst");
    std::fs::write(&offline, b"offline").expect("离线归档应可写入");

    cleanup_runtime_session_families(root.path(), &["s-payload".to_owned()])
        .expect("Session family 清理应成功");

    assert!(!payload_path.exists(), "外置正文目录必须随会话族回收");
    assert!(offline.exists(), "离线归档必须保留");
}

#[test]
fn expired_session_events_reclaim_exclusive_payload_after_commit() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "INSERT INTO session_runtimes VALUES('s-expire','s-expire','s-expire','/w','completed',NULL,NULL,0,0,0,1,1,1,1);
             INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref)
                 VALUES('s-expire',1,'g','turn','',1,'s-expire/1.json');",
        )
        .expect("测试数据应可写入");
    drop(connection);

    let archive_root = root
        .path()
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join("archive")
        .join("session-events");
    let payload_path = archive_root.join("s-expire").join("1.json");
    std::fs::create_dir_all(payload_path.parent().expect("父目录")).expect("目录应可创建");
    std::fs::write(&payload_path, b"{}").expect("外置正文应可写入");

    let mut connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(RUNTIME_DATABASE_FILE),
    )
    .expect("数据库应可重开");
    let counts = crate::cron_commands::runtime_cleanup_families::delete_expired_session_events(
        &mut connection,
        &archive_root,
        100,
        Instant::now() + Duration::from_secs(5),
    )
    .expect("定向事件清理应成功");
    drop(connection);

    assert_eq!(counts.session_events, 1);
    assert!(!payload_path.exists(), "提交后必须回收独占外置正文");
    assert!(
        !archive_root.join("s-expire").exists(),
        "空目录必须一并清理"
    );
}

#[test]
fn exhausted_budget_skips_session_event_deletion_without_archiving() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "INSERT INTO session_runtimes VALUES('s-budget','s-budget','s-budget','/w','completed',NULL,NULL,0,0,0,1,1,1,1);
             INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref)
                 VALUES('s-budget',1,'g','turn','',1,'s-budget/1.json');",
        )
        .expect("测试数据应可写入");
    drop(connection);

    let archive_root = root
        .path()
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join("archive")
        .join("session-events");
    let payload_path = archive_root.join("s-budget").join("1.json");
    std::fs::create_dir_all(payload_path.parent().expect("父目录")).expect("目录应可创建");
    std::fs::write(&payload_path, b"{}").expect("外置正文应可写入");

    let counts = run_runtime_cleanup_with_budget(root.path(), 100, false, Duration::ZERO)
        .expect("零预算清理应成功");

    assert_eq!(counts.session_events, 0, "归档耗尽预算时不得裸删事件");
    assert!(payload_path.exists(), "未归档的正文不得被回收");

    let connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(RUNTIME_DATABASE_FILE),
    )
    .expect("数据库应可重开");
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_events WHERE session_id = 's-budget'",
            [],
            |row| row.get(0),
        )
        .expect("统计应成功");
    assert_eq!(remaining, 1, "事件必须留待下一轮先归档再删除");
}

#[test]
fn dangling_payload_refs_are_repaired_without_touching_present_files() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "INSERT INTO session_runtimes VALUES('s-repair','s-repair','s-repair','/w','completed',NULL,NULL,0,0,0,0,300,1,1);
             INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref)
                 VALUES('s-repair',1,'g','turn','',200,'s-repair/1.json');
             INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref)
                 VALUES('s-repair',2,'g','turn','',210,'s-repair/2.json');",
        )
        .expect("测试数据应可写入");
    drop(connection);

    let archive_root = root
        .path()
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join("archive")
        .join("session-events");
    let present = archive_root.join("s-repair").join("2.json");
    std::fs::create_dir_all(present.parent().expect("父目录")).expect("目录应可创建");
    std::fs::write(&present, b"{\"ok\":true}").expect("外置正文应可写入");

    let mut connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(RUNTIME_DATABASE_FILE),
    )
    .expect("数据库应可重开");
    let repaired = crate::cron_commands::runtime_cleanup_families::repair_dangling_payload_refs(
        &mut connection,
        &archive_root,
        100,
        Instant::now() + Duration::from_secs(5),
    )
    .expect("悬空引用修复应成功");
    drop(connection);

    assert_eq!(repaired, 1, "只应修复缺失正文的那一行");
    assert!(present.exists(), "正文仍存在的行不得被改写成占位");
    let connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(RUNTIME_DATABASE_FILE),
    )
    .expect("数据库应可重开");
    let (missing_ref, missing_type): (Option<String>, String) = connection
        .query_row(
            "SELECT payload_ref, event_type FROM session_events WHERE session_id = 's-repair' AND sequence = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("查询应成功");
    assert!(missing_ref.is_none(), "悬空引用必须被置空");
    assert_eq!(missing_type, "turn", "event_type 不应被改写");
}
