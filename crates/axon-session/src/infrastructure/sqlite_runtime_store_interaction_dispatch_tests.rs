//! 执行侧派发记录的用例：这些行只作记录，绝不进入主 turn 的领取队列。
//!
//! 两条不变量各有用例兜住：领取按会话类型收口；派发记录不冒充用户渠道。

use super::SqliteRuntimeStore;
use crate::{
    InteractionQueryStore, InteractionStore, NewSessionRequest, RuntimeStore, SessionKind,
    SessionTrustScope,
};
use axon_domain::{
    AGENT_DISPATCH_SOURCE_CHANNEL, AudienceContext, AudienceKind, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};

use super::sqlite_runtime_store_interaction_fixtures::{interaction_record, test_session};

const FIRST_CLAIM_AT_MS: u64 = 20;
const LEASE_DURATION_MS: u64 = super::sqlite_runtime_store_interaction_tests::LEASE_DURATION_MS;

/// 领取按会话类型收口：只领主 turn 调度器真正驱动的那两类会话。
///
/// 子代理会话由父 turn 的执行段驱动、目标执行会话由执行侧驱动，它们的入站事实只作记录；
/// 一旦被这里领走，同一轮会在别人的会话里再跑一遍。
#[test]
fn claim_skips_sessions_the_primary_worker_does_not_drive() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let primary = test_session("01J8XQ7K3M4N5P6Q7R8S9T0V1W");
    let subagent = test_session("01J8XQ7K3M4N5P6Q7R8S9T0V2W");
    let cron = test_session("01J8XQ7K3M4N5P6Q7R8S9T0V3W");
    for (session, kind, agent_path) in [
        (&primary, SessionKind::Primary, None),
        (&subagent, SessionKind::Subagent, Some("root/task")),
        (&cron, SessionKind::Cron, None),
    ] {
        store
            .create_session_with(&NewSessionRequest {
                session_id: session.clone(),
                kind,
                trust_scope: SessionTrustScope::Private,
                workspace_id: None,
                root_session_id: (kind == SessionKind::Subagent).then(|| primary.clone()),
                agent_path: agent_path.map(str::to_owned),
                goal_id: None,
                execution_id: None,
                cwd: ".".to_owned(),
                now_ms: 10,
            })
            .expect("登记会话");
        assert!(
            store
                .append(&interaction_record(
                    session,
                    &format!("interaction-{kind:?}"),
                    &format!("message-{kind:?}"),
                    10,
                ))
                .expect("追加")
                .inserted
        );
    }

    let claimed = store
        .claim_processing(10, FIRST_CLAIM_AT_MS, LEASE_DURATION_MS)
        .expect("领取");
    let claimed_sessions = claimed
        .iter()
        .map(|lease| lease.interaction.session_id.as_str())
        .collect::<Vec<_>>();
    assert!(claimed_sessions.contains(&primary.as_str()));
    assert!(claimed_sessions.contains(&cron.as_str()));
    assert!(!claimed_sessions.contains(&subagent.as_str()));

    // 会话行缺失的历史车道仍要可领：迁移期不能因为兜底失败丢一轮输入。
    let orphan = test_session("session-orphan");
    store
        .append(&interaction_record(
            &orphan,
            "interaction-orphan",
            "message-orphan",
            11,
        ))
        .expect("追加");
    let claimed = store
        .claim_processing(10, FIRST_CLAIM_AT_MS, LEASE_DURATION_MS)
        .expect("再次领取");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].interaction.session_id, orphan);
}

/// 执行侧派发的记录不能冒充「用户最近用过的渠道」。
///
/// 子代理与目标执行的任务提示落的是执行子会话，渠道为 `agent`；若它被最近外部渠道
/// 扫描命中，完成通知与问答回投就会去找一条根本没有终端在听的投递目标。
#[test]
fn agent_dispatch_records_stay_out_of_external_channel_scan() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session = test_session("session-primary");
    store
        .create_session_with(&NewSessionRequest::primary(session.clone(), ".", 10))
        .expect("登记会话");

    store
        .append(&interaction_record(
            &session,
            "interaction-user",
            "message-user",
            10,
        ))
        .expect("追加用户入站");
    store
        .append(
            &InteractionRecord::new(
                session.clone(),
                InteractionId::new("interaction-dispatch").expect("交互 ID"),
                InteractionSource::new(
                    AGENT_DISPATCH_SOURCE_CHANNEL,
                    "agent-dispatch:parent:root/task:run-1",
                )
                .expect("来源"),
                ScopeRef::workspace("workspace-test").expect("作用域"),
                AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
                ReplyRoute::new(AGENT_DISPATCH_SOURCE_CHANNEL, "parent-session").expect("路由"),
                "子代理任务提示",
                20,
            )
            .expect("Interaction"),
        )
        .expect("追加派发记录");

    let latest = store
        .latest_external_interaction("workspace-test")
        .expect("读取最近外部交互")
        .expect("用户入站仍在");
    assert_eq!(latest.interaction_id.as_str(), "interaction-user");
}
