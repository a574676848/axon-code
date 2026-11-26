//! Runtime SQLite schema 校验与首次建库原子性测试。
//!
//! 两个库各自用 `PRAGMA user_version` 标记不兼容变更：只认当前版本，版本不一致或
//! 陌生库直接拒绝，不做补列与回填。

use std::sync::{Arc, Barrier};

use axon_domain::DaemonSessionId;

use super::SqliteRuntimeStore;
use crate::ports::RuntimeStore;

/// 并发首开者数量：足够多才让建库窗口稳定重叠，又不至于拖慢测试。
const CONCURRENT_OPENERS: usize = 8;

#[test]
fn legacy_database_without_version_is_rejected_without_creating_target_schema() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let control = store.path().to_path_buf();
    let legacy = rusqlite::Connection::open(&control).expect("打开旧数据库");
    legacy
        .execute_batch("CREATE TABLE legacy_goal_brain(goal_id TEXT PRIMARY KEY NOT NULL);")
        .expect("构造旧 schema");
    drop(legacy);

    assert!(
        store.initialize().is_err(),
        "没有版本标记的既有数据库必须直接拒绝"
    );

    let connection = rusqlite::Connection::open(&control).expect("复查数据库");
    let target_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='goals'",
            [],
            |row| row.get(0),
        )
        .expect("查询目标表");
    assert_eq!(target_tables, 0, "拒绝路径不得自动迁移或创建目标 schema");
}

#[test]
fn mismatched_schema_version_is_rejected() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let control = store.path().to_path_buf();
    let legacy = rusqlite::Connection::open(&control).expect("打开旧数据库");
    legacy
        .execute_batch(
            "CREATE TABLE legacy_goal_brain(goal_id TEXT PRIMARY KEY NOT NULL);
             PRAGMA user_version = 99;",
        )
        .expect("构造旧版本库");
    drop(legacy);

    assert!(store.initialize().is_err(), "版本不匹配必须直接拒绝");
}

/// 并发首次打开必须全部成功，且不得留下「有表无版本」的半成品库。
///
/// 本机 `axon run`、daemon 与子代理执行面会在同一进程内各自持有 `SqliteRuntimeStore`
/// 实例，并可能同时首次打开同一个库。DDL 批次很长、`user_version` 在批次末尾才写入，
/// 因此「检查 + 建库」若不是同一个写事务，后来的连接就会在「已建部分表、尚未写版本」
/// 的窗口里把正常建库误判为陌生库，并把半成品库永久留在该状态。本用例固化该判据。
#[test]
fn concurrent_first_open_builds_schema_atomically() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    let barrier = Arc::new(Barrier::new(CONCURRENT_OPENERS));
    let openers = (0..CONCURRENT_OPENERS)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let store = SqliteRuntimeStore::new(&path);
                // 所有线程在各自取到 store 后同时进入首次初始化，最大化建库窗口重叠。
                barrier.wait();
                store.initialize().map_err(|error| error.to_string())
            })
        })
        .collect::<Vec<_>>();

    for opener in openers {
        let result = opener.join().expect("并发首次打开的线程不应 panic");
        assert!(result.is_ok(), "并发首次打开必须成功，实际：{result:?}");
    }

    // 建库后版本与业务表同时可见：再打开无需重建，也不会被判为陌生库。
    let reopened = SqliteRuntimeStore::new(&path);
    reopened.initialize().expect("建库完成后再次打开必须成功");
    reopened.initialize().expect("重复初始化必须幂等成功");
}

#[test]
fn legacy_database_is_rejected_instead_of_migrated() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let control = store.path().to_path_buf();
    // 目标态只认当前 schema：旧版本库必须拒绝打开，不做补列与回填。
    {
        let connection = rusqlite::Connection::open(&control).expect("打开旧库");
        connection
            .execute_batch(
                "PRAGMA user_version = 1;
                 CREATE TABLE inbox (
                    interaction_id TEXT PRIMARY KEY NOT NULL
                 );",
            )
            .expect("旧库 schema");
    }
    store.initialize().expect_err("旧版本库必须拒绝");
}

/// 库文件被清空或替换后，重新初始化必须重新建表。
///
/// 「这个路径在本进程里初始化过」只能证明过去，不能证明磁盘现状：库文件被删除、清空或
/// 替换后路径不变，但表已经没了。按路径跳过建库会让后续写入撞上
/// `no such table: session_metrics`（Linux CI 上临时目录被提前回收时的真实故障形态）。
/// 本用例不依赖线程调度，直接构造「路径不变、表全没」的库。
#[test]
fn recreated_empty_database_is_initialized_again() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    let first = SqliteRuntimeStore::new(&path);
    first.initialize().expect("首次初始化");
    let control = first.path().to_path_buf();
    let outbox = first.outbox_path().to_path_buf();
    let session_id = DaemonSessionId::new("session-schema-recreated").expect("会话 id");
    first.create_session(&session_id, ".", 10).expect("建会话");
    // 先释放连接池：Windows 上仍被打开的文件删不掉，也就无法构造「文件被替换」。
    drop(first);
    std::fs::remove_dir_all(directory.path()).expect("删除库所在目录");
    std::fs::create_dir_all(directory.path()).expect("重建库所在目录");
    // 原地重建两个空库：路径不变、表全没。
    std::fs::write(&control, b"").expect("重建空控制面库");
    std::fs::write(&outbox, b"").expect("重建空队列库");

    let reopened = SqliteRuntimeStore::new(&path);
    reopened.initialize().expect("空库必须重新建表");
    // 断言落在故障点上：投影读取不得再以 `no such table` 失败。
    let metrics = reopened
        .session_metrics(&session_id)
        .expect("投影读取必须可用");
    assert!(metrics.is_none(), "重建后的空库不应有历史投影");
}

/// 版本不匹配：对外只给产品语义，版本号留在 Debug 与日志里。
#[test]
fn schema_version_mismatch_reports_product_message_with_versions_in_debug() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let control = store.path().to_path_buf();
    let legacy = rusqlite::Connection::open(&control).expect("打开旧数据库");
    legacy
        .execute_batch(
            "CREATE TABLE legacy_goal_brain(goal_id TEXT PRIMARY KEY NOT NULL);
             PRAGMA user_version = 99;",
        )
        .expect("构造旧版本库");
    drop(legacy);

    let error = store.initialize().expect_err("版本不匹配必须失败");
    let message = error.to_string();
    assert!(
        message.contains("运行态数据与当前程序版本不一致"),
        "用户可见文案必须是产品语义：{message}"
    );
    assert!(
        message.contains("重启"),
        "文案必须给出可执行动作：{message}"
    );
    assert!(
        !message.contains("read-only"),
        "不得回 rusqlite 的误导文案：{message}"
    );
    let debug = format!("{error:?}");
    assert!(
        debug.contains("99"),
        "原始版本必须留在 Debug 供日志取证：{debug}"
    );
}

/// 低版本库（有版本标记、缺新表）应自动补齐新增表，且既有行一条不丢。
#[test]
fn a_lower_version_database_is_migrated_with_its_rows_intact() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    store.initialize().expect("建库");
    let control = store.path().to_path_buf();
    {
        let connection = rusqlite::Connection::open(&control).expect("打开控制面库");
        connection
            .execute(
                "INSERT INTO session_metrics(session_id, updated_at_ms) VALUES('keep-me', 1)",
                [],
            )
            .expect("写一行待保留数据");
        // 回到 15 版本并删掉本轮新增的表：等价于升级前的存量库。
        connection
            .execute_batch(
                "DROP TABLE projection_state; DROP TABLE projection_log_identity; \
                 PRAGMA user_version = 15;",
            )
            .expect("构造旧版本库");
    }

    let migrated = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    migrated.initialize().expect("低版本库必须自动迁移");
    let version: i64 = migrated
        .with_read_connection(|connection| {
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))
        })
        .expect("读版本");
    assert_eq!(version, 17, "迁移后版本号必须推进到当前版本");
    let kept: i64 = migrated
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM session_metrics WHERE session_id='keep-me'",
                [],
                |row| row.get(0),
            )
        })
        .expect("读保留数据");
    assert_eq!(kept, 1, "迁移不得丢既有数据");
    let projection_rows: i64 = migrated
        .with_read_connection(|connection| {
            connection.query_row("SELECT COUNT(*) FROM projection_state", [], |row| {
                row.get(0)
            })
        })
        .expect("新表必须存在");
    assert_eq!(projection_rows, 0);
}
