//! Runtime SQLite schema 校验与首次建库原子性测试。
//!
//! 两个库各自用 `PRAGMA user_version` 标记不兼容变更：只认当前版本，版本不一致或
//! 陌生库直接拒绝，不做补列与回填。

use std::sync::{Arc, Barrier};

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
