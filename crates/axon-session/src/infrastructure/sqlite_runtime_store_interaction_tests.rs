//! SQLite Interaction 持久化与执行所有权测试。

use super::SqliteRuntimeStore;
use crate::{InteractionQueryStore, InteractionStore, RuntimeStore, interaction_session_lane_id};
use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, InteractionStatus, ReplyRoute, RuntimeHostId, ScopeRef,
};

const FIRST_CLAIM_AT_MS: u64 = 20;
const RELEASE_AT_MS: u64 = 30;
const RECLAIM_AFTER_RELEASE_MS: u64 = 31;
const EXPIRED_RECLAIM_AT_MS: u64 = 121;
const SETTLE_AT_MS: u64 = 130;
const VERIFY_TERMINAL_AT_MS: u64 = 140;
const LEASE_DURATION_MS: u64 = 100;

fn sample_interaction() -> InteractionRecord {
    interaction_record("interaction-test", "message-test", 10)
}

fn interaction_record(
    interaction_id: &str,
    message_id: &str,
    received_at_ms: u64,
) -> InteractionRecord {
    InteractionRecord::new(
        InteractionId::new(interaction_id).expect("交互 ID"),
        InteractionSource::new("tui", message_id).expect("来源"),
        ScopeRef::workspace("workspace-test").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", "terminal-test").expect("路由"),
        "检查状态",
        received_at_ms,
    )
    .expect("Interaction")
}

/// 构造指定复合 lane（Workspace + 渠道 + 会话表面）的 Interaction。
fn interaction_in_lane(
    interaction_id: &str,
    workspace: &str,
    destination: &str,
    received_at_ms: u64,
) -> InteractionRecord {
    InteractionRecord::new(
        InteractionId::new(interaction_id).expect("交互 ID"),
        InteractionSource::new("tui", format!("message-{interaction_id}")).expect("来源"),
        ScopeRef::workspace(workspace).expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", destination).expect("路由"),
        "检查状态",
        received_at_ms,
    )
    .expect("Interaction")
}

#[test]
fn interaction_attachments_round_trip_through_every_read_path() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut interaction = interaction_record("interaction-attachment", "message-attachment", 10);
    interaction.attachments = vec![
        "/var/axon/media/image/a.png".to_owned(),
        "/var/axon/media/file/b.pdf".to_owned(),
    ];
    assert!(store.append(&interaction).expect("追加").inserted);

    // 每条读取路径都必须回读附件：遗漏任一 SELECT 都会在这里暴露为列错位。
    let by_id = store
        .get_interaction(&interaction.interaction_id)
        .expect("按 ID 读取")
        .expect("记录应存在");
    assert_eq!(by_id.attachments, interaction.attachments);
    let latest = store
        .latest_external_interaction("workspace-test")
        .expect("读取最近外部交互")
        .expect("记录应存在");
    assert_eq!(latest.attachments, interaction.attachments);
    let latest_any = store
        .latest_external_interaction_any()
        .expect("读取任意最近外部交互")
        .expect("记录应存在");
    assert_eq!(latest_any.attachments, interaction.attachments);
    let by_channel = store
        .latest_external_interaction_for_channel("workspace-test", "tui")
        .expect("按渠道读取")
        .expect("记录应存在");
    assert_eq!(by_channel.attachments, interaction.attachments);

    let claimed = store
        .claim_processing(1, FIRST_CLAIM_AT_MS, LEASE_DURATION_MS)
        .expect("领取");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].interaction.attachments, interaction.attachments);

    // 第二条记录用于覆盖按 ID 领取路径（第一条已被 lease 持有）。
    let mut second = interaction_record("interaction-attachment-2", "message-attachment-2", 11);
    second.attachments = vec!["/var/axon/media/image/c.png".to_owned()];
    assert!(store.append(&second).expect("追加第二条").inserted);
    let claimed_by_id = store
        .claim_processing_by_id(&second.interaction_id, FIRST_CLAIM_AT_MS, LEASE_DURATION_MS)
        .expect("按 ID 领取")
        .expect("应可领取");
    assert_eq!(claimed_by_id.interaction.attachments, second.attachments);
    let latest_processing = store
        .latest_processing_external_interaction("workspace-test")
        .expect("读取最近处理中交互")
        .expect("记录应存在");
    assert_eq!(latest_processing.attachments, second.attachments);
    let processing_by_channel = store
        .latest_processing_interaction_for_channel("workspace-test", "tui")
        .expect("按渠道读取处理中交互")
        .expect("记录应存在");
    assert_eq!(processing_by_channel.attachments, second.attachments);
    let processing_by_route = store
        .latest_processing_interaction_for_route("workspace-test", "tui", "terminal-test")
        .expect("按路由读取处理中交互")
        .expect("记录应存在");
    assert_eq!(processing_by_route.attachments, second.attachments);
}

#[test]
fn schema_initialization_adds_attachments_column_to_existing_database() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    // identity 从全新库读取，避免与生产常量重复字面量：identity 变更时本测试会随之更新。
    let identity = {
        let seed_dir = axon_test_support::test_temp_dir();
        let seed_path = seed_dir.path().join("seed.sqlite3");
        let seed = SqliteRuntimeStore::new(&seed_path);
        seed.initialize().expect("初始化 seed 库");
        let connection = rusqlite::Connection::open(&seed_path).expect("打开 seed 库");
        connection
            .query_row("SELECT identity FROM jarvis_schema_identity LIMIT 1", [], {
                |row| row.get::<_, String>(0)
            })
            .expect("读取 identity")
    };
    // 构造变更前的既有库：identity 有效、jarvis_interactions 已存在但没有 attachments_json。
    {
        let connection = rusqlite::Connection::open(&path).expect("打开既有库");
        connection
            .execute_batch(&format!(
                "CREATE TABLE jarvis_schema_identity (identity TEXT PRIMARY KEY NOT NULL);
                 INSERT INTO jarvis_schema_identity(identity) VALUES('{identity}');
                 CREATE TABLE jarvis_interactions (
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
                 );"
            ))
            .expect("既有库 schema");
    }
    let store = SqliteRuntimeStore::new(path);
    let mut interaction = interaction_record("interaction-migrated", "message-migrated", 10);
    interaction.attachments = vec!["/var/axon/media/image/migrated.png".to_owned()];
    assert!(store.append(&interaction).expect("迁移后可写入").inserted);
    let read = store
        .get_interaction(&interaction.interaction_id)
        .expect("读取")
        .expect("记录应存在");
    assert_eq!(read.attachments, interaction.attachments);
}

#[test]
fn interactions_are_idempotent_and_terminalize_expired_processing_leases() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("interaction-recovery-host").expect("host"),
            1,
            None,
            1,
            1_000,
        )
        .expect("host lease");
    let interaction = sample_interaction();
    assert!(store.append(&interaction).expect("首次追加").inserted);
    assert!(!store.append(&interaction).expect("重复追加").inserted);

    let first = store
        .claim_processing(1, FIRST_CLAIM_AT_MS, LEASE_DURATION_MS)
        .expect("领取");
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].interaction.status,
        axon_domain::InteractionStatus::Processing
    );
    assert!(
        store
            .claim_processing(1, 30, LEASE_DURATION_MS)
            .expect("未过期不可重复领取")
            .is_empty()
    );

    let interrupted = store
        .quarantine_interrupted_interactions(&host, 121, 10)
        .expect("收敛中断 Interaction");
    assert!(interrupted.is_empty());
    let recovered = store
        .get_interaction(&first[0].interaction.interaction_id)
        .expect("读取 Interaction")
        .expect("Interaction");
    assert_eq!(recovered.status, InteractionStatus::Interrupted);
    assert!(
        store
            .claim_processing(1, 140, LEASE_DURATION_MS)
            .expect("终态不可领取")
            .is_empty()
    );
}

#[test]
fn interrupted_interaction_appends_durable_session_and_ledger_events() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("interaction-event-host").expect("host"),
            1,
            None,
            1,
            1_000,
        )
        .expect("host lease");
    let interaction = sample_interaction();
    store.append(&interaction).expect("追加 Interaction");
    let lane_id = interaction_session_lane_id(&interaction).expect("lane");
    let session_id = DaemonSessionId::new(lane_id).expect("Session ID");
    store.create_session(&session_id, ".", 1).expect("Session");
    let lease = store
        .claim_processing_by_id(&interaction.interaction_id, 20, 100)
        .expect("领取")
        .expect("lease");

    let records = store
        .quarantine_interrupted_interactions(&host, 121, 10)
        .expect("收敛中断 Interaction");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, session_id);
    assert_eq!(records[0].event_type, "interaction_interrupted");
    assert!(records[0].payload_json.contains("interaction-test"));

    let runtime = store.get_session(&session_id).expect("Session runtime");
    assert_eq!(runtime.last_event_sequence, 1);
    let ledger_count: i64 = store
        .connection()
        .expect("connection")
        .query_row(
            "SELECT COUNT(*) FROM jarvis_events WHERE idempotency_key=?1",
            ["interaction.interrupted:interaction-test"],
            |row| row.get(0),
        )
        .expect("ledger event");
    assert_eq!(ledger_count, 1);
    assert_eq!(
        store
            .get_interaction(&lease.interaction.interaction_id)
            .expect("读取 Interaction")
            .expect("Interaction")
            .status,
        InteractionStatus::Interrupted
    );
    assert!(
        store
            .quarantine_interrupted_interactions(&host, 122, 10)
            .expect("重复收敛应幂等")
            .is_empty()
    );
}

#[test]
fn claim_keeps_arrival_order_across_queued_and_expired_records() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let earliest = interaction_record("interaction-earliest", "message-earliest", 10);
    let middle = interaction_record("interaction-middle", "message-middle", 40);
    let latest = interaction_record("interaction-latest", "message-latest", 70);
    store.append(&earliest).expect("追加最早记录");
    store.append(&middle).expect("追加中间记录");
    store.append(&latest).expect("追加最新记录");

    // 最早记录先被领取，lease 过期后交由中断恢复收敛；中间与最新记录仍在排队。
    store
        .claim_processing(1, FIRST_CLAIM_AT_MS, LEASE_DURATION_MS)
        .expect("首次领取最早记录");

    let claimed = store
        .claim_processing(3, EXPIRED_RECLAIM_AT_MS, LEASE_DURATION_MS)
        .expect("只领取排队记录");
    let order = claimed
        .iter()
        .map(|lease| lease.interaction.interaction_id.as_str().to_owned())
        .collect::<Vec<_>>();

    assert_eq!(order, vec!["interaction-middle", "interaction-latest"]);
}

#[test]
fn claim_keeps_lane_exclusive_while_a_turn_is_in_flight() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    store
        .append(&interaction_in_lane(
            "interaction-lane-a-1",
            "workspace-a",
            "terminal-a",
            10,
        ))
        .expect("追加");
    store
        .append(&interaction_in_lane(
            "interaction-lane-a-2",
            "workspace-a",
            "terminal-a",
            20,
        ))
        .expect("追加");
    store
        .append(&interaction_in_lane(
            "interaction-lane-b-1",
            "workspace-b",
            "terminal-b",
            30,
        ))
        .expect("追加");

    let first = store
        .claim_processing(1, FIRST_CLAIM_AT_MS, LEASE_DURATION_MS)
        .expect("首次领取");
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].interaction.interaction_id.as_str(),
        "interaction-lane-a-1"
    );

    // 首个 lane 仍在 lease 内：它的后续记录必须留在队列，不能与在途 turn 并发执行。
    let next = store
        .claim_processing(10, RECLAIM_AFTER_RELEASE_MS, LEASE_DURATION_MS)
        .expect("再次领取");
    let ids = next
        .iter()
        .map(|lease| lease.interaction.interaction_id.as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["interaction-lane-b-1"]);

    // 在途 turn 结束后，同 lane 的后续记录重新可领取。
    store
        .mark_understood(
            &first[0].interaction.interaction_id,
            &first[0].lease_token,
            SETTLE_AT_MS,
        )
        .expect("提交终态");
    let resumed = store
        .claim_processing(10, VERIFY_TERMINAL_AT_MS, LEASE_DURATION_MS)
        .expect("恢复领取");
    let resumed_ids = resumed
        .iter()
        .map(|lease| lease.interaction.interaction_id.as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(resumed_ids, ["interaction-lane-a-2"]);
}

#[test]
fn claim_by_id_and_renew_keep_single_executor_ownership() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let interaction = sample_interaction();
    store.append(&interaction).expect("追加 Interaction");

    let lease = store
        .claim_processing_by_id(
            &interaction.interaction_id,
            FIRST_CLAIM_AT_MS,
            LEASE_DURATION_MS,
        )
        .expect("按 ID 领取")
        .expect("首次领取应成功");
    assert!(
        store
            .claim_processing_by_id(&interaction.interaction_id, 30, LEASE_DURATION_MS)
            .expect("检查并发领取")
            .is_none()
    );

    let renewed_until = store
        .renew_processing(
            &interaction.interaction_id,
            &lease.lease_token,
            100,
            LEASE_DURATION_MS,
        )
        .expect("续租");
    assert_eq!(renewed_until, 200);
    assert!(
        store
            .claim_processing_by_id(&interaction.interaction_id, 121, LEASE_DURATION_MS)
            .expect("续租后检查所有权")
            .is_none()
    );
}

#[test]
fn released_processing_lease_can_be_claimed_immediately() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let interaction = sample_interaction();
    store.append(&interaction).expect("追加 Interaction");

    let first = store
        .claim_processing_by_id(
            &interaction.interaction_id,
            FIRST_CLAIM_AT_MS,
            LEASE_DURATION_MS,
        )
        .expect("首次领取")
        .expect("首次 lease");
    store
        .release_processing(
            &interaction.interaction_id,
            &first.lease_token,
            RELEASE_AT_MS,
        )
        .expect("释放 processing lease");

    let reclaimed = store
        .claim_processing_by_id(
            &interaction.interaction_id,
            RECLAIM_AFTER_RELEASE_MS,
            LEASE_DURATION_MS,
        )
        .expect("释放后重新领取")
        .expect("应立即获得新 lease");
    assert_ne!(reclaimed.lease_token, first.lease_token);
}

#[test]
fn stale_lease_token_cannot_release_reclaimed_interaction() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let interaction = sample_interaction();
    store.append(&interaction).expect("追加 Interaction");

    let expired = store
        .claim_processing_by_id(
            &interaction.interaction_id,
            FIRST_CLAIM_AT_MS,
            LEASE_DURATION_MS,
        )
        .expect("首次领取")
        .expect("首次 lease");
    let current = store
        .claim_processing_by_id(
            &interaction.interaction_id,
            EXPIRED_RECLAIM_AT_MS,
            LEASE_DURATION_MS,
        )
        .expect("过期重领")
        .expect("新 lease");

    assert!(
        store
            .release_processing(
                &interaction.interaction_id,
                &expired.lease_token,
                SETTLE_AT_MS,
            )
            .is_err()
    );
    store
        .mark_understood(
            &interaction.interaction_id,
            &current.lease_token,
            SETTLE_AT_MS,
        )
        .expect("旧 token 未破坏当前 lease");
}

#[test]
fn understood_interaction_cannot_be_claimed_again() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let interaction = sample_interaction();
    store.append(&interaction).expect("追加 Interaction");
    let lease = store
        .claim_processing_by_id(
            &interaction.interaction_id,
            FIRST_CLAIM_AT_MS,
            LEASE_DURATION_MS,
        )
        .expect("首次领取")
        .expect("首次 lease");
    store
        .mark_understood(
            &interaction.interaction_id,
            &lease.lease_token,
            SETTLE_AT_MS,
        )
        .expect("提交 understood");

    assert!(
        store
            .claim_processing_by_id(
                &interaction.interaction_id,
                VERIFY_TERMINAL_AT_MS,
                LEASE_DURATION_MS,
            )
            .expect("终态领取检查")
            .is_none()
    );
}

#[test]
fn append_rejects_forged_non_received_status() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut interaction = sample_interaction();
    interaction.status = InteractionStatus::Understood;

    let error = store.append(&interaction).expect_err("入站不得伪造终态");
    assert!(error.to_string().contains("received"));
}

#[test]
fn expired_lease_token_cannot_renew_or_settle_reclaimed_interaction() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let interaction = sample_interaction();
    store.append(&interaction).expect("追加 Interaction");
    let expired = store
        .claim_processing_by_id(
            &interaction.interaction_id,
            FIRST_CLAIM_AT_MS,
            LEASE_DURATION_MS,
        )
        .expect("首次领取")
        .expect("首次 lease");
    let current = store
        .claim_processing_by_id(
            &interaction.interaction_id,
            EXPIRED_RECLAIM_AT_MS,
            LEASE_DURATION_MS,
        )
        .expect("过期重领")
        .expect("新 lease");

    assert_ne!(current.lease_token, expired.lease_token);
    assert!(
        store
            .renew_processing(
                &interaction.interaction_id,
                &expired.lease_token,
                SETTLE_AT_MS,
                LEASE_DURATION_MS,
            )
            .is_err()
    );
    assert!(
        store
            .mark_understood(
                &interaction.interaction_id,
                &expired.lease_token,
                SETTLE_AT_MS,
            )
            .is_err()
    );
    store
        .mark_understood(
            &interaction.interaction_id,
            &current.lease_token,
            SETTLE_AT_MS + 1,
        )
        .expect("当前 lease 可提交终态");
}

#[test]
fn workspace_channel_queries_find_latest_history_and_active_turn() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let first = interaction_record("interaction-first", "message-first", 10);
    let mut second = interaction_record("interaction-second", "message-second", 20);
    second.reply_route = ReplyRoute::new("tui", "terminal-second").expect("路由");
    store.append(&first).expect("追加第一条");
    store.append(&second).expect("追加第二条");

    let latest = store
        .latest_external_interaction_for_channel("workspace-test", "tui")
        .expect("读取最近 TUI Interaction")
        .expect("应存在最近 Interaction");
    assert_eq!(latest.interaction_id.as_str(), "interaction-second");
    assert_eq!(latest.reply_route.destination(), "terminal-second");

    store
        .claim_processing_by_id(&first.interaction_id, 30, LEASE_DURATION_MS)
        .expect("领取活动 Interaction")
        .expect("第一条应进入 processing");
    let active = store
        .latest_processing_interaction_for_channel("workspace-test", "tui")
        .expect("读取 workspace 活动 Interaction")
        .expect("应存在活动 Interaction");
    assert_eq!(active.interaction_id.as_str(), "interaction-first");
    assert!(
        store
            .latest_processing_interaction_for_channel("workspace-test", "feishu")
            .expect("按渠道隔离查询")
            .is_none()
    );
}

#[test]
fn expired_interaction_lease_cannot_append_late_runtime_events() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("late-event-host").expect("host"),
            1,
            None,
            1,
            1_000,
        )
        .expect("host lease");
    let interaction = sample_interaction();
    store.append(&interaction).expect("追加 Interaction");
    let session_id = DaemonSessionId::new(interaction_session_lane_id(&interaction).expect("lane"))
        .expect("Session ID");
    store.create_session(&session_id, ".", 1).expect("Session");
    let lease = store
        .claim_processing_by_id(&interaction.interaction_id, 20, LEASE_DURATION_MS)
        .expect("领取")
        .expect("lease");

    store
        .append_interaction_events(
            &session_id,
            &host,
            &interaction.interaction_id,
            &lease.lease_token,
            &[("assistant_delta".to_owned(), "{}".to_owned())],
            30,
        )
        .expect("有效 lease 可追加");
    store
        .quarantine_interrupted_interactions(&host, 121, 10)
        .expect("supervisor 收口")
        .retain(|event| event.session_id == session_id);
    let error = store
        .append_interaction_events(
            &session_id,
            &host,
            &interaction.interaction_id,
            &lease.lease_token,
            &[("streaming_completed".to_owned(), "{}".to_owned())],
            122,
        )
        .expect_err("旧 lease 不得追加迟到事件");
    assert!(matches!(error, crate::RuntimeStoreError::Fenced));
    assert_eq!(
        store
            .get_session(&session_id)
            .expect("读取 Session")
            .last_event_sequence,
        2
    );
}
