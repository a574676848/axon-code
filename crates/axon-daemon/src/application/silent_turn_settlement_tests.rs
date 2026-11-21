//! 静默轮次判定与显式收口的应用层测试。

use super::*;
use crate::application::silent_turn_settlement::{
    SettleSilentTurnRequest, SilentTurnReason, SilentTurnSettlementService,
};
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, ReplyRoute, ScopeRef,
};
use axon_session::{
    InteractionQueryStore, InteractionStore, InteractionStoreError, RuntimeStore,
    SqliteRuntimeStore, conclusive_work_items_without_terminal_kind_sql,
};

#[test]
fn silent_turn_with_stale_lease_offers_server_side_settle() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = DaemonSessionId::new("session-silent-turn").expect("会话");
    store.create_session(&session_id, ".", 10).expect("会话");
    let interaction = interaction_record(&session_id, "interaction-silent-turn");
    store.append(&interaction).expect("入站事实");
    store
        .claim_processing_by_id(&interaction.interaction_id, 20, 100)
        .expect("领取")
        .expect("租约");
    let service = SilentTurnSettlementService::new(store.clone());

    // 租约 100ms，过期末尾再看：这一轮没有任何执行者。
    let snapshot = service.inspect(&session_id, 1_000, None).expect("判死快照");
    assert_eq!(snapshot.turns.len(), 1, "在途轮次必须被判死");
    let verdict = snapshot.turns.first().expect("轮次");
    assert_eq!(verdict.reason, Some(SilentTurnReason::LeaseExpired));
    assert_eq!(verdict.lease_expires_at_ms, Some(120));
    assert_eq!(
        snapshot.settle_offers().count(),
        1,
        "判定必须给出服务端收口的入口"
    );

    let request = SettleSilentTurnRequest::new(session_id.clone(), 1_000);
    let receipt = service.settle(&request).expect("服务端收口");
    assert_eq!(
        receipt.settled,
        vec![interaction.interaction_id.clone()],
        "必须真正收口这一轮"
    );
    assert!(receipt.kept_running.is_empty());
    assert_eq!(
        turn_status(&store, &session_id, interaction.interaction_id.as_str()),
        axon_domain::InteractionStatus::Interrupted,
        "收口后索引必须读得出终态"
    );
    assert_eq!(
        ledger_event_count(&store, "interaction.interrupted"),
        1,
        "收口必须留下既有事件名的流水账"
    );
    assert_eq!(conclusion_violations(&store), 0, "INV-2 必须为零行");

    // 重复收口不能再写一次终态：幂等由存储层的状态守卫保证。
    let again = service.settle(&request).expect("重复收口");
    assert!(again.settled.is_empty());
    assert_eq!(ledger_event_count(&store, "interaction.interrupted"), 1);
}

#[test]
fn turn_without_first_token_frame_is_settled_after_deadline() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = DaemonSessionId::new("session-first-token").expect("会话");
    store.create_session(&session_id, ".", 10).expect("会话");
    let interaction = interaction_record(&session_id, "interaction-first-token");
    store.append(&interaction).expect("入站事实");
    // 租约远未到期：执行者还活着，但领取之后一个事件帧都没写过。
    store
        .claim_processing_by_id(&interaction.interaction_id, 20, 1_000_000)
        .expect("领取")
        .expect("租约");
    let service = SilentTurnSettlementService::new(store.clone());

    // 期限未到：仍在跑，不得收口。
    let running = service
        .inspect(&session_id, 20 + 60_000, Some(120_000))
        .expect("判死快照");
    assert_eq!(running.settle_offers().count(), 0);

    let request = SettleSilentTurnRequest::new(session_id.clone(), 20 + 120_000)
        .with_first_token_deadline_ms(120_000);
    let snapshot = service
        .inspect(&session_id, request.now_ms, Some(120_000))
        .expect("判死快照");
    assert_eq!(
        snapshot.turns.first().and_then(|turn| turn.reason),
        Some(SilentTurnReason::FirstTokenDeadlineExceeded),
        "首字超时必须收敛为可收口的判死结论"
    );
    let receipt = service.settle(&request).expect("服务端收口");
    assert_eq!(receipt.settled.len(), 1);
    assert_eq!(
        turn_status(&store, &session_id, interaction.interaction_id.as_str()),
        axon_domain::InteractionStatus::Interrupted
    );
}

#[test]
fn zero_deadline_is_rejected_instead_of_settling_everything() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = DaemonSessionId::new("session-zero-deadline").expect("会话");
    store.create_session(&session_id, ".", 10).expect("会话");
    let service = SilentTurnSettlementService::new(store.clone());

    let error = service
        .inspect(&session_id, 10, Some(0))
        .expect_err("0 期限必须被拒绝");
    assert_eq!(error, SilentTurnSettlementError::InvalidDeadline);
}

fn interaction_record(session_id: &DaemonSessionId, interaction_id: &str) -> InteractionRecord {
    InteractionRecord::new(
        session_id.clone(),
        InteractionId::new(interaction_id).expect("交互标识"),
        InteractionSource::new("tui", format!("message-{interaction_id}")).expect("来源"),
        ScopeRef::workspace("workspace-test").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", "terminal-test").expect("路由"),
        "检查静默轮次",
        10,
    )
    .expect("Interaction")
}

fn turn_status(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    interaction_id: &str,
) -> axon_domain::InteractionStatus {
    store
        .interaction_turn_heads(session_id.as_str(), 10)
        .expect("轮次头")
        .into_iter()
        .find(|head| head.interaction_id == interaction_id)
        .expect("轮次必须存在")
        .status
}

fn ledger_event_count(store: &SqliteRuntimeStore, event_type: &str) -> i64 {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM ledger_events WHERE event_type=?1",
                [event_type],
                |row| row.get(0),
            )
        })
        .expect("流水账计数")
}

fn conclusion_violations(store: &SqliteRuntimeStore) -> i64 {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                &conclusive_work_items_without_terminal_kind_sql(),
                [],
                |row| row.get(0),
            )
        })
        .expect("INV-2 检查")
}

/// 收口失败时的错误类型必须可构造，避免网关层把它降级成不可分类的存储错误。
#[test]
fn store_errors_keep_their_own_variant() {
    let error = SilentTurnSettlementError::from(InteractionStoreError::Storage("x".to_owned()));
    assert!(matches!(
        error,
        SilentTurnSettlementError::Store(InteractionStoreError::Storage(_))
    ));
}
