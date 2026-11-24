//! 轮次来源渠道的覆盖探针（W7.1）与重放重建（W7.2）。
//!
//! 渠道是轮次事实：四个入口各跑一轮，「本轮从哪个端进入」必须落到
//! `turn_metrics.origin_channel` 与会话级分布 `channel_mix_json` 上，且重放能重建同一份分布。

use std::collections::BTreeMap;

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, RuntimeHostId, ScopeRef,
};
use axon_eventlog::{RuntimeEventKind, TurnMetrics, TurnOutcome};

use super::SqliteRuntimeStore;
use crate::ports::{EventLogStore as _, InteractionStore as _, RuntimeStore as _};

/// 四个入口：前三个各自带客户端，外部事件没有客户端、来源就是渠道本身。
///
/// 第三列是期望落库的来源：客户端优先，没有客户端时退回 `source_channel`。
const ENTRIES: [(&str, Option<&str>); 4] = [
    ("desktop", Some("desktop")),
    ("cli", Some("cli")),
    ("tui", Some("tui")),
    ("jira", None),
];

const LEASE_DURATION_MS: u64 = 60_000;

fn turn_metrics(turn_id: &str) -> TurnMetrics {
    TurnMetrics {
        turn_id: turn_id.to_owned(),
        ttft_ms: Some(120),
        active_ms: 800,
        provider_duration_ms: 400,
        tool_ms: 100,
        input_tokens: 100,
        output_tokens: 40,
        reasoning_tokens: 10,
        cache_read_tokens: 5,
        cache_write_tokens: 3,
        generation_speed_tps: Some(40.5),
        background_jobs_started: 0,
        background_jobs_running: 0,
        outcome: TurnOutcome::Succeeded,
        tool_calls: 1,
    }
}

fn raw_interaction(
    session_id: &DaemonSessionId,
    index: usize,
    channel: &str,
    client: Option<&str>,
) -> InteractionRecord {
    let mut source = InteractionSource::new(channel, format!("message-{index}")).expect("来源");
    if let Some(client) = client {
        source = source.with_client(client).expect("客户端");
    }
    InteractionRecord::new(
        session_id.clone(),
        InteractionId::new(format!("01J8Z0000000000000000100{index:02}")).expect("交互标识"),
        source,
        ScopeRef::workspace("workspace-a").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new(channel, "destination-a").expect("回复路由"),
        "检查进度",
        10,
    )
    .expect("Interaction")
}

/// 写一条入站与它的一轮指标，返回该轮落库的来源。
fn record_turn(
    store: &SqliteRuntimeStore,
    host: &crate::domain::runtime::DaemonHostRecord,
    session_id: &DaemonSessionId,
    interaction: &InteractionRecord,
    turn_id: &str,
    now_ms: u64,
) -> Option<String> {
    store.append(interaction).expect("追加 Interaction");
    let lease = store
        .claim_processing_by_id(&interaction.interaction_id, now_ms, LEASE_DURATION_MS)
        .expect("领取")
        .expect("租约");
    let payload = serde_json::to_string(&turn_metrics(turn_id)).expect("指标序列化");
    store
        .append_interaction_events(
            session_id,
            host,
            &interaction.interaction_id,
            &lease.lease_token,
            &[("turn_metrics".to_owned(), payload)],
            now_ms + 1,
        )
        .expect("追加轮次指标");
    let records =
        axon_eventlog::read_after(&store.event_log(session_id).path(), 0, 32).expect("读事件日志");
    records
        .iter()
        .find(|record| record.event_type == RuntimeEventKind::TurnMetrics)
        .and_then(|record| record.origin_channel.clone())
}

#[test]
fn origin_channel_covers_all_four_inbound_entries() {
    for (index, (channel, client)) in ENTRIES.into_iter().enumerate() {
        let directory = axon_test_support::test_temp_path(&format!("channel-origin-entry-{index}"));
        let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
        let host = store
            .claim_host(
                &RuntimeHostId::new("channel-origin-host").expect("host"),
                1,
                None,
                10,
                LEASE_DURATION_MS,
            )
            .expect("host 租约");
        let session_id =
            DaemonSessionId::new(format!("01J8Z0000000000000000200{index:02}")).expect("会话标识");
        store.create_session(&session_id, ".", 10).expect("会话");
        let interaction = raw_interaction(&session_id, index, channel, client);

        let origin = record_turn(&store, &host, &session_id, &interaction, "turn-1", 20);

        assert_eq!(
            origin.as_deref(),
            Some(channel),
            "入口 {channel} 的轮次来源必须是它自己"
        );
        let snapshot = store
            .session_metrics(&session_id)
            .expect("读投影")
            .expect("投影应存在");
        assert_eq!(
            snapshot.channel_mix.get(channel),
            Some(&1),
            "入口 {channel} 的会话级分布应记一条：{:?}",
            snapshot.channel_mix
        );
        assert!(
            !snapshot.channel_mix.contains_key("internal"),
            "有来源的轮次不得记成 internal：{:?}",
            snapshot.channel_mix
        );
    }
}

#[test]
fn rebuild_restores_the_same_channel_mix_after_real_interactions() {
    let directory = axon_test_support::test_temp_path("channel-origin-rebuild");
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("channel-origin-host").expect("host"),
            1,
            None,
            10,
            LEASE_DURATION_MS,
        )
        .expect("host 租约");
    let session_id = DaemonSessionId::new("01J8Z00000000000000003000").expect("会话标识");
    store.create_session(&session_id, ".", 10).expect("会话");

    let mut expected: BTreeMap<String, u64> = BTreeMap::new();
    for (index, (channel, client)) in ENTRIES.into_iter().take(3).enumerate() {
        let interaction = raw_interaction(&session_id, 10 + index, channel, client);
        record_turn(
            &store,
            &host,
            &session_id,
            &interaction,
            &format!("turn-{index}"),
            20 + u64::try_from(index).expect("序号") * 10,
        );
        *expected.entry(channel.to_owned()).or_insert(0) += 1;
    }
    let live = store
        .session_metrics(&session_id)
        .expect("读投影")
        .expect("投影应存在");
    assert_eq!(live.channel_mix, expected, "在线累计应与逐轮来源一致");

    // 投影是可重建的派生数据：整体重放事件日志必须得到同一份分布，且重复重建幂等。
    let rebuilt = store
        .rebuild_session_metrics(&session_id, 200)
        .expect("重建投影");
    assert_eq!(rebuilt.channel_mix, expected, "重放后分布必须一致");
    let again = store
        .rebuild_session_metrics(&session_id, 300)
        .expect("再次重建");
    assert_eq!(again.channel_mix, expected, "重复重建必须幂等");
}
