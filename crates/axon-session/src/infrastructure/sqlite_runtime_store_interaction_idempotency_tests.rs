//! 交互入站的幂等契约：来源键是身份，标识由服务端生成。

use super::SqliteRuntimeStore;
use super::sqlite_runtime_store_interaction_fixtures::sample_interaction;
use super::sqlite_runtime_store_interaction_tests::LEASE_DURATION_MS;
use crate::InteractionStore;

/// 重放方生成的标识会被丢弃：幂等身份是来源键，回执指回库里那一条。
#[test]
fn replay_with_a_regenerated_id_keeps_the_stored_identity() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut interaction = sample_interaction();
    let stored = store.append(&interaction).expect("首次追加").interaction_id;

    // 服务端标识是 ULID，重放方不可能复现：它带着自己生成的一枚标识再来一次。
    interaction.interaction_id =
        axon_domain::InteractionId::new("01J8ZQ7K3M4N5P6Q7R8S9T0V3A").expect("交互标识");
    let replay = store.append(&interaction).expect("重放应命中已有行");
    assert!(!replay.inserted, "重放不得新写一行");
    assert_eq!(replay.interaction_id, stored, "回执必须指回库里的标识");
    assert_eq!(
        store
            .claim_processing(10, 20, LEASE_DURATION_MS)
            .expect("读取")
            .len(),
        1,
        "同一条事件只允许一行 Interaction"
    );
}

/// 同一个来源键换了正文：这是两条不同的事实，必须拒绝而不是静默覆盖。
#[test]
fn reusing_a_source_key_with_different_content_is_rejected() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut interaction = sample_interaction();
    store.append(&interaction).expect("首次追加");

    interaction.interaction_id =
        axon_domain::InteractionId::new("01J8ZQ7K3M4N5P6Q7R8S9T0V3B").expect("交互标识");
    interaction.content = "换了一段正文".to_owned();
    let error = store
        .append(&interaction)
        .expect_err("来源键相同但内容不一致必须拒绝");
    assert!(
        matches!(error, crate::InteractionStoreError::Conflict(_)),
        "应报内容冲突：{error:?}"
    );
    assert_eq!(
        store
            .claim_processing(10, 20, LEASE_DURATION_MS)
            .expect("读取")
            .len(),
        1,
        "被拒的重放不得写入第二行"
    );
}
