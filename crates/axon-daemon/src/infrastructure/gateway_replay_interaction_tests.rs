//! 交互历史页的按键取帧契约：一页只解压目标交互自己的帧。
//!
//! 与 `gateway_replay_tests.rs` 分开成文：那边锁定的是「回放补齐到 durable 尾」与
//! 订阅游标，这里只锁定历史页的取帧范围与续读游标。

use super::*;
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, ReplyRoute, RuntimeHostId, ScopeRef, SessionRuntimeStatus,
};
use axon_session::{EventLogStore as _, InteractionStore, RuntimeStore, SqliteRuntimeStore};

/// 两个交互共用同一条会话：会话标识由作用域、受众与路由派生，这三项相同即是同一车道。
struct Fixture {
    store: SqliteRuntimeStore,
    host: axon_session::DaemonHostRecord,
    session_id: axon_domain::DaemonSessionId,
    first: InteractionId,
    second: InteractionId,
}

fn fixture(directory: &std::path::Path) -> Fixture {
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("interaction-page-host").expect("Host ID"),
            1,
            None,
            10,
            10_000,
        )
        .expect("注册 Host");
    let first = InteractionId::new("interaction-page-first").expect("首个交互");
    let second = InteractionId::new("interaction-page-second").expect("第二个交互");
    let mut session_id = None;
    for interaction_id in [&first, &second] {
        let record = InteractionRecord::new(
            axon_domain::DaemonSessionId::new("01J8XQ7K3M4N5P6Q7R8S9T0V1W").expect("合法会话 ID"),
            interaction_id.clone(),
            InteractionSource::new("desktop", format!("message-{}", interaction_id.as_str()))
                .expect("有效来源"),
            ScopeRef::workspace("workspace-interaction-page").expect("有效作用域"),
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            ReplyRoute::new("desktop", "workbench-interaction-page").expect("有效路由"),
            "按键取帧",
            12,
        )
        .expect("Interaction 有效");
        let derived = crate::application::primary_session_id_for_interaction(&record)
            .expect("由 Interaction 派生会话");
        if session_id.is_none() {
            store
                .create_session(&derived, ".", 10)
                .expect("创建 Session");
            store
                .set_session_status(&derived, &host, SessionRuntimeStatus::Running, 11)
                .expect("启动 Session");
            session_id = Some(derived);
        }
        store.append(&record).expect("写入 Interaction");
    }
    Fixture {
        store,
        host,
        session_id: session_id.expect("会话已创建"),
        first,
        second,
    }
}

/// 领取一次 processing lease：同一交互只有一份租约，交错写入必须复用它。
fn claim_lease(fixture: &Fixture, interaction_id: &InteractionId) -> String {
    fixture
        .store
        .claim_processing_by_id(interaction_id, 20, 10_000)
        .expect("领取不应失败")
        .expect("应有 processing lease")
        .lease_token
        .clone()
}

/// 给一条交互追加 `count` 条事件（每次都带归属列，走真实写入路径）。
fn append_events(
    fixture: &Fixture,
    interaction_id: &InteractionId,
    lease_token: &str,
    count: u64,
    base: u64,
) {
    for offset in 0..count {
        fixture
            .store
            .append_interaction_events(
                &fixture.session_id,
                &fixture.host,
                interaction_id,
                lease_token,
                &[(
                    "wire_event".to_owned(),
                    format!(
                        "{{\"interactionId\":\"{}\",\"offset\":{offset}}}",
                        interaction_id.as_str()
                    ),
                )],
                base.saturating_add(offset),
            )
            .expect("追加目标交互事件");
    }
}

#[test]
fn history_page_pages_only_the_target_interaction() {
    let directory = axon_test_support::test_temp_dir();
    let fixture = fixture(directory.path());
    let first_lease = claim_lease(&fixture, &fixture.first);
    let second_lease = claim_lease(&fixture, &fixture.second);
    // 两条交互交替写入：整条会话里帧是交错的，按键取帧必须仍然只返回目标交互。
    append_events(&fixture, &fixture.first, &first_lease, 6, 30);
    append_events(&fixture, &fixture.second, &second_lease, 6, 40);
    append_events(&fixture, &fixture.first, &first_lease, 6, 50);

    let mut after = 0_u64;
    let mut seen = Vec::new();
    let mut pages = 0_u32;
    loop {
        let page = interaction_history_page(&fixture.store, &fixture.first, after, 5, false)
            .expect("历史页");
        pages = pages.saturating_add(1);
        assert!(pages <= 6, "续读必须有界，实际 {pages} 页");
        let payloads: Vec<&str> = page
            .events
            .iter()
            .map(|event| event.payload_json.as_str())
            .collect();
        for payload in payloads {
            assert!(
                payload.contains(fixture.first.as_str()),
                "历史页不得混入别条交互的帧：{payload}"
            );
            assert!(
                !payload.contains(fixture.second.as_str()),
                "历史页不得混入别条交互的帧：{payload}"
            );
        }
        seen.push(page.events.len());
        after = page.next_after;
        if !page.has_more {
            break;
        }
    }

    assert_eq!(
        seen.iter().sum::<usize>(),
        12,
        "两条交互各写 6 条，目标交互必须恰好翻出 12 条：{seen:?}"
    );
    assert_eq!(
        *seen.last().expect("至少一页"),
        2,
        "最后一页应只剩 2 条，{seen:?}"
    );
}

#[test]
fn history_page_reports_no_more_when_the_interaction_is_short() {
    let directory = axon_test_support::test_temp_dir();
    let fixture = fixture(directory.path());
    let first_lease = claim_lease(&fixture, &fixture.first);
    let second_lease = claim_lease(&fixture, &fixture.second);
    append_events(&fixture, &fixture.first, &first_lease, 3, 30);
    // 别条交互在目标交互之后写了很多帧：整会话尾序号远大于目标交互的末条，
    // 历史页的 has_more 必须按目标交互判断，不能拿整会话尾序号猜。
    append_events(&fixture, &fixture.second, &second_lease, 20, 60);

    let page =
        interaction_history_page(&fixture.store, &fixture.first, 0, 50, false).expect("历史页");
    assert_eq!(page.events.len(), 3);
    assert!(
        !page.has_more,
        "目标交互已经到底，后续帧属于别条交互，不得再要求续读"
    );
    assert!(
        page.tail_sequence > page.next_after,
        "整会话尾序号确实更大，说明 has_more 不是由它推导的"
    );
}
