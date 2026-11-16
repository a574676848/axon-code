//! Runtime SQLite schema 校验、首次建库原子性与追加式列迁移测试。
//!
//! 两个库各自用 `PRAGMA user_version` 标记不兼容变更：版本一致时既建库路径只补
//! 幂等 DDL，版本不一致或陌生库必须直接拒绝，避免「编译通过、运行时表不存在」。

use std::sync::{Arc, Barrier};

use super::SqliteRuntimeStore;
use crate::ports::{InteractionQueryStore, InteractionStore, RuntimeStore};
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, ReplyRoute, ScopeRef,
};

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
fn schema_initialization_adds_additive_columns_to_existing_database() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let control = store.path().to_path_buf();
    // 构造变更前的既有库：版本一致、inbox 已存在但没有追加列。
    {
        let connection = rusqlite::Connection::open(&control).expect("打开既有库");
        connection
            .execute_batch(
                "PRAGMA user_version = 1;
                 CREATE TABLE inbox (
                    interaction_id TEXT PRIMARY KEY NOT NULL,
                    source_channel TEXT NOT NULL,
                    source_message_id TEXT NOT NULL,
                    scope_json TEXT NOT NULL,
                    audience_json TEXT NOT NULL,
                    reply_route_json TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    received_at_ms INTEGER NOT NULL,
                    lease_token TEXT,
                    lease_expires_at_ms INTEGER,
                    updated_at_ms INTEGER NOT NULL
                 );",
            )
            .expect("既有库 schema");
    }
    let mut interaction = InteractionRecord::new(
        InteractionId::new("interaction-migrated").expect("合法 ID"),
        InteractionSource::new("tui", "message-migrated").expect("合法来源"),
        ScopeRef::workspace("workspace-migrated").expect("合法作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", "terminal-migrated").expect("合法路由"),
        "迁移后继续",
        10,
    )
    .expect("Interaction 应创建");
    interaction.attachments = vec!["media/image/migrated.png".to_owned()];
    interaction.model = Some("balanced".to_owned());
    assert!(store.append(&interaction).expect("迁移后可写入").inserted);
    let read = store
        .get_interaction(&interaction.interaction_id)
        .expect("读取")
        .expect("记录应存在");
    assert_eq!(read.attachments, interaction.attachments);
    assert_eq!(read.model, interaction.model);
}
