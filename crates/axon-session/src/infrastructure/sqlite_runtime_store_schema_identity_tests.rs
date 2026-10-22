//! Runtime SQLite schema identity 拒绝路径与首次建库原子性测试。

use std::sync::{Arc, Barrier};

use super::SqliteRuntimeStore;
use crate::ports::RuntimeStore;

/// 并发首开者数量：足够多才让建库窗口稳定重叠，又不至于拖慢测试。
const CONCURRENT_OPENERS: usize = 8;

#[test]
fn legacy_database_without_identity_is_rejected_without_creating_target_schema() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    let legacy = rusqlite::Connection::open(&path).expect("打开旧数据库");
    legacy
        .execute_batch("CREATE TABLE legacy_goal_brain(goal_id TEXT PRIMARY KEY NOT NULL);")
        .expect("构造旧 schema");
    drop(legacy);

    let store = SqliteRuntimeStore::new(&path);
    assert!(
        store.initialize().is_err(),
        "无 identity 的既有数据库必须直接拒绝"
    );

    let connection = rusqlite::Connection::open(&path).expect("复查数据库");
    let target_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='jarvis_goals'",
            [],
            |row| row.get(0),
        )
        .expect("查询目标表");
    assert_eq!(target_tables, 0, "拒绝路径不得自动迁移或创建目标 schema");
}

#[test]
fn mismatched_schema_identity_is_rejected() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    let legacy = rusqlite::Connection::open(&path).expect("打开旧数据库");
    legacy
        .execute_batch(
            "CREATE TABLE jarvis_schema_identity(identity TEXT PRIMARY KEY NOT NULL);
             INSERT INTO jarvis_schema_identity(identity) VALUES('jarvis-runtime-v1');",
        )
        .expect("构造旧 identity");
    drop(legacy);

    let store = SqliteRuntimeStore::new(&path);
    assert!(store.initialize().is_err(), "identity 不匹配必须直接拒绝");
}

/// 并发首次打开必须全部成功，且不得留下「有表无 identity」的半成品库。
///
/// 本机 `axon run`、daemon 与子代理执行面会在同一进程内各自持有 `SqliteRuntimeStore`
/// 实例，并可能同时首次打开同一个 `runtime.sqlite3`。DDL 批次很长、identity 行在批次
/// 末尾才写入，因此「检查 + 建库」若不是同一个写事务，后来的连接就会在
/// 「已建部分表、尚未写 identity」的窗口里把正常建库误判为陌生库，并把半成品库永久
/// 留在该状态。本用例固化该判据。
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

    // 建库后 identity 与业务表同时可见：再打开无需重建，也不会被判为陌生库。
    let reopened = SqliteRuntimeStore::new(&path);
    reopened.initialize().expect("建库完成后再次打开必须成功");
    reopened.initialize().expect("重复初始化必须幂等成功");
}
