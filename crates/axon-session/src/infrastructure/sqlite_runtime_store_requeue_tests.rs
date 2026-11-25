//! 运行中追加的重投：入站侧早已结算成 understood，运行侧却可能一笔都没取走。
//!
//! 判据三条：重投后必须能被主 turn 调度器重新领取；只认 understood 这一种结算；
//! 重复调用幂等，绝不在在途表里制造第二条工作项。

use super::SqliteRuntimeStore;
use super::sqlite_runtime_store_interaction_fixtures::{interaction_record, test_session};
use crate::ports::InteractionStore as _;

/// 与生产同量级的租约窗口，只为让领取条件里的时间比较有意义。
const LEASE_DURATION_MS: u64 = 100;

#[test]
fn requeued_steered_interaction_is_claimable_again() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session = test_session("session-requeue");
    let interaction = interaction_record(&session, "interaction-requeue", "message-requeue", 10);
    assert!(store.append(&interaction).expect("追加").inserted);

    // 入站侧命中活跃 turn：立刻领取并结算成 understood。
    let lease = store
        .claim_processing_by_id(&interaction.interaction_id, 20, LEASE_DURATION_MS)
        .expect("按 ID 领取")
        .expect("首次领取必须成功");
    store
        .mark_understood(&interaction.interaction_id, &lease.lease_token, 30)
        .expect("结算 understood");

    // 结算即归档：调度器的领取查询用「没有历史行」判定从未处理过，因此这时领不到。
    assert!(
        store
            .claim_processing(1, 40, LEASE_DURATION_MS)
            .expect("领取")
            .is_empty(),
        "已结算的入站事实不得被再次领取"
    );

    assert!(
        store
            .requeue_steered(&interaction.interaction_id, 50)
            .expect("重投"),
        "运行侧没取走的追加必须能被重投"
    );

    let claimed = store
        .claim_processing(1, 60, LEASE_DURATION_MS)
        .expect("重投后领取");
    assert_eq!(claimed.len(), 1, "重投之后必须重新可领");
    assert_eq!(
        claimed[0].interaction.interaction_id, interaction.interaction_id,
        "领到的必须正是被重投的那一条"
    );
    assert_eq!(
        claimed[0].interaction.content, interaction.content,
        "重投不得改动正文"
    );
}

#[test]
fn requeue_only_reverses_understood_settlements() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session = test_session("session-requeue-rejected");
    let interaction = interaction_record(&session, "interaction-rejected", "message-rejected", 10);
    assert!(store.append(&interaction).expect("追加").inserted);
    let lease = store
        .claim_processing_by_id(&interaction.interaction_id, 20, LEASE_DURATION_MS)
        .expect("按 ID 领取")
        .expect("首次领取必须成功");
    store
        .mark_rejected(&interaction.interaction_id, &lease.lease_token, 30)
        .expect("结算 rejected");

    assert!(
        !store
            .requeue_steered(&interaction.interaction_id, 40)
            .expect("重投判定"),
        "被拒绝是另一类结论，重投它等于替用户改写结论"
    );
    assert!(
        store
            .claim_processing(1, 50, LEASE_DURATION_MS)
            .expect("领取")
            .is_empty(),
        "被拒绝的入站事实仍然不可领"
    );
}

#[test]
fn requeue_is_idempotent_and_never_stacks_work_items() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session = test_session("session-requeue-twice");
    let interaction = interaction_record(&session, "interaction-twice", "message-twice", 10);
    assert!(store.append(&interaction).expect("追加").inserted);
    let lease = store
        .claim_processing_by_id(&interaction.interaction_id, 20, LEASE_DURATION_MS)
        .expect("按 ID 领取")
        .expect("首次领取必须成功");
    store
        .mark_understood(&interaction.interaction_id, &lease.lease_token, 30)
        .expect("结算 understood");

    assert!(
        store
            .requeue_steered(&interaction.interaction_id, 40)
            .expect("首次重投")
    );
    assert!(
        !store
            .requeue_steered(&interaction.interaction_id, 50)
            .expect("二次重投"),
        "重复重投必须是幂等的空操作"
    );

    // 在途表里只允许一条：多插一条会让同一句话跑两轮。
    let claimed = store
        .claim_processing(10, 60, LEASE_DURATION_MS)
        .expect("领取");
    assert_eq!(claimed.len(), 1, "同一条入站事实只能有一个工作项");
}
