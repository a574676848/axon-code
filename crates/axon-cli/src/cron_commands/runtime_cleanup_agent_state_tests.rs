//! mailbox 消息与 continuation 的运行态保留回收测试。
//!
//! 判据：越过**自身保留窗口**的协作事实由 Cron 回收，仍可投递 / 仍可装配的事实必须
//! 原样保留；dry-run 统计与实际删除同口径（同一份保留谓词）。

use axon_session::{
    AgentContinuationRecord, AgentContinuationStore, AgentMailboxMessageKind,
    AgentMailboxMessageRecord, AgentMailboxStore, RuntimeStore, SqliteRuntimeStore,
};

use super::*;

const OWNER_ID: &str = "session-owner";
/// 已越过保留窗口的到期时刻（早于 Cron 水位）。
const EXPIRED_AT_MS: u64 = 10;
/// 仍在保留窗口内的到期时刻（晚于 Cron 水位）。
const LIVE_AT_MS: u64 = 10_000;
/// Cron 清理水位：判定「自身窗口 + Cron 水位」双双越过。
const CLEANUP_CUTOFF_MS: u64 = 100;

fn open_runtime_store(root: &Path) -> SqliteRuntimeStore {
    let store = SqliteRuntimeStore::from_root(root.join(axon_config::AXON_RUNTIME_DATA_DIR));
    store.initialize().expect("测试 schema 应可初始化");
    store
}

fn message(message_id: &str, key: &str, expires_at_ms: u64) -> AgentMailboxMessageRecord {
    AgentMailboxMessageRecord::pending(
        message_id,
        key,
        OWNER_ID,
        "/root",
        "/root/review",
        AgentMailboxMessageKind::Collab,
        "协作消息",
        false,
        EXPIRED_AT_MS,
        expires_at_ms,
    )
}

fn continuation(continuation_id: &str, key: &str, expires_at_ms: u64) -> AgentContinuationRecord {
    AgentContinuationRecord::pending(
        continuation_id,
        key,
        OWNER_ID,
        OWNER_ID,
        "child-session",
        "/root/review",
        "completed",
        "子代理正常结束",
        "审查完成",
        EXPIRED_AT_MS,
        expires_at_ms,
    )
}

#[test]
fn expired_agent_state_is_reclaimed_while_live_state_is_kept() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path();
    let store = open_runtime_store(root);

    // 越过保留窗口：含仍处于 pending（待投递 / 待装配）的记录。
    store
        .append_agent_message(&message("m-expired", "k-m-expired", EXPIRED_AT_MS))
        .expect("消息应可写入");
    store
        .append_continuation(&continuation("c-expired", "k-c-expired", EXPIRED_AT_MS))
        .expect("continuation 应可写入");
    // 仍在窗口内：必须原样保留。
    store
        .append_agent_message(&message("m-live", "k-m-live", LIVE_AT_MS))
        .expect("消息应可写入");
    store
        .append_continuation(&continuation("c-live", "k-c-live", LIVE_AT_MS))
        .expect("continuation 应可写入");

    let counted = count_runtime_cleanup(root, CLEANUP_CUTOFF_MS).expect("dry-run 统计");
    assert_eq!(counted.agent_messages, 1, "dry-run 必须只统计越窗消息");
    assert_eq!(
        counted.continuations, 1,
        "dry-run 必须只统计越窗 continuation"
    );

    let deleted = cleanup_runtime_data(root, CLEANUP_CUTOFF_MS).expect("清理应成功");
    assert_eq!(
        deleted.agent_messages, counted.agent_messages,
        "dry-run 与实际删除必须同口径"
    );
    assert_eq!(
        deleted.continuations, counted.continuations,
        "dry-run 与实际删除必须同口径"
    );

    assert!(
        store.agent_message("m-expired").expect("查询").is_none(),
        "越窗消息必须被回收"
    );
    assert!(
        store.continuation("c-expired").expect("查询").is_none(),
        "越窗 continuation 必须被回收"
    );
    assert!(
        store.agent_message("m-live").expect("查询").is_some(),
        "未越窗消息不得被回收"
    );
    assert!(
        store.continuation("c-live").expect("查询").is_some(),
        "未越窗 continuation 不得被回收"
    );
}

#[test]
fn cleanup_is_idempotent_when_nothing_is_expired() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path();
    let store = open_runtime_store(root);
    store
        .append_agent_message(&message("m-live", "k-m-live", LIVE_AT_MS))
        .expect("消息应可写入");

    let first = cleanup_runtime_data(root, CLEANUP_CUTOFF_MS).expect("清理应成功");
    assert_eq!(first.agent_messages, 0);
    assert_eq!(first.continuations, 0);
    // 幂等：第二轮不得再删任何行，也不得删除仍在窗口内的事实。
    let second = cleanup_runtime_data(root, CLEANUP_CUTOFF_MS).expect("清理应成功");
    assert_eq!(second.agent_messages, 0);
    assert_eq!(second.continuations, 0);
    assert!(store.agent_message("m-live").expect("查询").is_some());
}
