//! `axon debug mailbox` 的查询与渲染测试（合同 §5.3.2 的状态查询路径）。

use axon_session::{
    AgentMailboxMessageKind, AgentMailboxMessageRecord, AgentMailboxStore, SqliteRuntimeStore,
};

use super::{
    CONTENT_PREVIEW_CHARS, MailboxReport, build_report, preview, render_text, resolve_state,
};
use crate::{DebugMailboxCommand, DebugMailboxState};

const OWNER_ID: &str = "session-owner-1";
const CREATED_AT_MS: u64 = 1_000;
const RETENTION_MS: u64 = 600_000;

struct MailboxFixture {
    store: SqliteRuntimeStore,
    // 字段顺序即析构顺序：临时目录必须最后析构，`store` 先释放 SQLite 句柄，目录才删得掉。
    _directory: axon_test_support::TestTempDir,
}

fn mailbox_fixture() -> MailboxFixture {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    MailboxFixture {
        store,
        _directory: directory,
    }
}

fn command(state: DebugMailboxState, limit: u32) -> DebugMailboxCommand {
    DebugMailboxCommand {
        owner: Some(OWNER_ID.to_owned()),
        recipient: None,
        state,
        limit,
    }
}

fn message(message_id: &str, key: &str, recipient: &str) -> AgentMailboxMessageRecord {
    AgentMailboxMessageRecord::pending(
        message_id,
        key,
        OWNER_ID,
        "/root",
        recipient,
        AgentMailboxMessageKind::Collab,
        "继续处理评审项",
        false,
        CREATED_AT_MS,
        CREATED_AT_MS + RETENTION_MS,
    )
}

fn seeded_store() -> MailboxFixture {
    let fixture = mailbox_fixture();
    fixture
        .store
        .append_agent_message(&message("msg-pending", "key-pending", "/root/review"))
        .expect("落盘");
    let mut delivered = message("msg-delivered", "key-delivered", "/root/review");
    delivered.attempt_count = 1;
    fixture
        .store
        .append_agent_message(&delivered)
        .expect("落盘");
    fixture
        .store
        .deliver_agent_message("msg-delivered", CREATED_AT_MS + 10)
        .expect("投递");
    let mut failed = message("msg-failed", "key-failed", "/root/review");
    failed.state = axon_session::AgentMailboxMessageState::Pending;
    fixture.store.append_agent_message(&failed).expect("落盘");
    fixture
        .store
        .record_agent_message_failure(
            "msg-failed",
            "agent_control:recipient_unavailable",
            CREATED_AT_MS + 20,
            axon_session::AgentMailboxRetryPolicy {
                backoff_ms: 1_000,
                max_backoff_ms: 8_000,
                max_attempts: 8,
            },
        )
        .expect("记账");
    fixture
}

#[test]
fn state_filter_maps_command_variants_to_storage_states() {
    assert_eq!(
        resolve_state(DebugMailboxState::Pending),
        Some(axon_session::AgentMailboxMessageState::Pending)
    );
    assert_eq!(
        resolve_state(DebugMailboxState::Delivered),
        Some(axon_session::AgentMailboxMessageState::Delivered)
    );
    assert_eq!(
        resolve_state(DebugMailboxState::Read),
        Some(axon_session::AgentMailboxMessageState::Read),
        "读态必须可查询：读确认与投递确认分离后，未读与已读要能分别过滤"
    );
    assert_eq!(
        resolve_state(DebugMailboxState::Expired),
        Some(axon_session::AgentMailboxMessageState::Expired)
    );
    assert_eq!(
        resolve_state(DebugMailboxState::Any),
        None,
        "Any 表示不加状态条件"
    );
}

#[test]
fn preview_bounds_long_content_and_marks_the_cut() {
    let short = "继续处理";
    assert_eq!(preview(short), short);

    let long = "长".repeat(CONTENT_PREVIEW_CHARS + 10);
    let rendered = preview(&long);
    assert_eq!(rendered.chars().count(), CONTENT_PREVIEW_CHARS + 1);
    assert!(rendered.ends_with('…'));
}

#[test]
fn report_defaults_to_pending_and_carries_retry_facts() {
    let fixture = seeded_store();
    let report =
        build_report(&fixture.store, &command(DebugMailboxState::Pending, 20)).expect("查询");

    assert_eq!(report.count, 2, "默认只列待投递的消息");
    assert_eq!(report.state_filter, "pending");
    let failed = report
        .messages
        .iter()
        .find(|message| message.message_id == "msg-failed")
        .expect("含失败记账的消息");
    assert_eq!(failed.attempt_count, 1);
    assert_eq!(
        failed.last_error.as_deref(),
        Some("agent_control:recipient_unavailable")
    );
    assert_eq!(failed.retry_at_ms, Some(CREATED_AT_MS + 20 + 1_000));
}

#[test]
fn report_lists_every_state_when_unfiltered() {
    let fixture = seeded_store();
    let report = build_report(&fixture.store, &command(DebugMailboxState::Any, 20)).expect("查询");

    assert_eq!(report.count, 3);
    assert!(
        report
            .messages
            .iter()
            .any(|message| message.state == "delivered")
    );
}

#[test]
fn report_honours_the_limit() {
    let fixture = seeded_store();
    let report = build_report(&fixture.store, &command(DebugMailboxState::Any, 1)).expect("查询");
    assert_eq!(report.count, 1);
}

#[test]
fn recipient_filter_narrows_the_result() {
    let fixture = seeded_store();
    let mut command = command(DebugMailboxState::Any, 20);
    command.recipient = Some("/root/other".to_owned());
    let report = build_report(&fixture.store, &command).expect("查询");
    assert_eq!(report.count, 0, "别的接收方不得被列进本次查询");
}

#[test]
fn text_rendering_surfaces_the_facts_needed_to_diagnose_delivery() {
    let fixture = seeded_store();
    let report =
        build_report(&fixture.store, &command(DebugMailboxState::Pending, 20)).expect("查询");
    let text = render_text(&report);

    assert!(text.starts_with("mailbox: 2 条"));
    assert!(text.contains("msg-failed"));
    assert!(text.contains("agent_control:recipient_unavailable"));
    assert!(text.contains("retryAt="));
    assert!(text.contains("正文：继续处理评审项"));
}

#[test]
fn empty_result_is_reported_explicitly() {
    let fixture = mailbox_fixture();
    let report: MailboxReport =
        build_report(&fixture.store, &command(DebugMailboxState::Pending, 20)).expect("查询");
    assert_eq!(report.count, 0);
    assert!(render_text(&report).contains("无匹配消息"));
}
