//! 会话累计指标的投影回源验收。
//!
//! 三件事各一条用例：投影落后时按日志补齐水位之后的轮次；投影不落后时一次事件日志都不读；
//! 水位停在批首序号时，早已投影完的最后一批不得被重复累加。全部在临时运行态根里跑。

use axon_domain::{DaemonSessionId, InteractionId, RuntimeHostId};
use axon_eventlog::{RuntimeEventKind, RuntimeEventRecord};
use serde_json::{Value, json};

use super::super::SqliteRuntimeStore;
use crate::domain::DaemonHostRecord;
use crate::ports::{EventLogStore, RuntimeStore};

const SESSION_ID: &str = "session-projection-fallback";

fn fixture() -> (SqliteRuntimeStore, DaemonSessionId, DaemonHostRecord) {
    let directory = axon_test_support::test_temp_path("projection-fallback");
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    store.initialize().expect("初始化运行态库");
    let host = store
        .claim_host(
            &RuntimeHostId::new("host-projection-fallback").expect("Host ID"),
            1,
            None,
            1_000,
            60_000,
        )
        .expect("领取 Host lease");
    let session_id = DaemonSessionId::new(SESSION_ID).expect("Session ID");
    store
        .create_session(&session_id, ".", 1_000)
        .expect("建立会话");
    (store, session_id, host)
}

/// 一份完整的 `turn_metrics` 载荷；缺字段会让反序列化整体失败。
fn metrics_payload(turn_id: &str, active_ms: u64, input_tokens: u64, output_tokens: u64) -> Value {
    json!({
        "turnId": turn_id,
        "activeMs": active_ms,
        "toolCalls": 1,
        "inputTokens": input_tokens,
        "outputTokens": output_tokens,
        "reasoningTokens": 0,
        "cacheReadTokens": 0,
        "cacheWriteTokens": 0,
        "providerDurationMs": active_ms,
        "toolMs": 0,
        "backgroundJobsStarted": 0,
        "backgroundJobsRunning": 0,
    })
}

/// 走正常写路径追加一批事件，返回本批末条序号。
fn append_batch(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    interaction_id: &str,
    events: &[(String, String)],
    now_ms: u64,
) -> u64 {
    let interaction = InteractionId::new(interaction_id).expect("Interaction ID");
    let written = store
        .append_attributed_events(session_id, host, &interaction, events, now_ms)
        .expect("追加事件");
    written.last().map_or(0, |record| record.sequence)
}

/// 只写事件日志、不投影：模拟「写路径已返回，投影 worker 还没消费这一批」。
fn append_to_log_only(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    interaction_id: &str,
    metrics: &Value,
    sequence: u64,
) {
    let log = store.event_log(session_id);
    let record = RuntimeEventRecord::new(
        sequence,
        1_000 + sequence,
        RuntimeEventKind::TurnMetrics,
        metrics.clone(),
    )
    .with_interaction(interaction_id);
    axon_eventlog::append_once(&log, sequence, 1_000 + sequence, &[record]).expect("直接写日志");
}

/// 把控制面事件水位推到指定序号。
fn mark_event_tail(store: &SqliteRuntimeStore, session_id: &DaemonSessionId, tail: u64) {
    store
        .with_read_connection(|connection| {
            connection.execute(
                "UPDATE sessions SET last_event_sequence = ?1 WHERE session_id = ?2",
                rusqlite::params![tail as i64, session_id.as_str()],
            )
        })
        .expect("推事件水位");
}

/// 投影落后时，回源必须补上只在日志里的那一轮。
#[test]
fn projection_behind_fills_the_unprojected_turn() {
    let (store, session_id, host) = fixture();
    // 两批各一条事件：批首序号每一步都等于事件尾，投影因此是「追平」状态。
    let completed = [(
        "turn_completed".to_owned(),
        json!({ "interactionId": "interaction-a", "response": "结论" }).to_string(),
    )];
    append_batch(
        &store,
        &session_id,
        &host,
        "interaction-a",
        &completed,
        1_100,
    );
    let projected = metrics_payload("interaction-a", 900, 1_000, 100);
    let metrics_batch = [("turn_metrics".to_owned(), projected.to_string())];
    let tail = append_batch(
        &store,
        &session_id,
        &host,
        "interaction-a",
        &metrics_batch,
        1_200,
    );
    let before = store
        .session_metrics(&session_id)
        .expect("读取投影")
        .expect("投影必须已落地");
    assert_eq!(before.turn_count, 1);
    assert_eq!(before.total_input_tokens, 1_000);

    // 第三条事件只写日志：控制面水位已推进，投影水位还停在上一批。
    let unprojected = metrics_payload("interaction-b", 400, 500, 25);
    append_to_log_only(&store, &session_id, "interaction-b", &unprojected, tail + 1);
    mark_event_tail(&store, &session_id, tail + 1);

    let merged = store
        .session_metrics_with_fallback(&session_id)
        .expect("读取累计指标")
        .expect("必须给出读数");
    assert_eq!(merged.turn_count, 2, "落后时必须把日志里那一轮补进来");
    assert_eq!(merged.total_input_tokens, 1_500);
    assert_eq!(merged.total_output_tokens, 125);
    assert_eq!(merged.total_active_ms, 1_300);

    // 读路径不回写投影表：投影仍是落后状态，补齐只对本次读取生效。
    let after = store
        .session_metrics(&session_id)
        .expect("读取投影")
        .expect("投影行仍在");
    assert_eq!(after.turn_count, 1, "回源结果不得写回投影表");
    assert_eq!(after.total_input_tokens, 1_000);
}

/// 投影不落后时一次事件日志都不读，结果与投影完全一致。
#[test]
fn projection_caught_up_skips_the_event_log() {
    let (store, session_id, host) = fixture();
    let completed = [(
        "turn_completed".to_owned(),
        json!({ "interactionId": "interaction-a", "response": "结论" }).to_string(),
    )];
    append_batch(
        &store,
        &session_id,
        &host,
        "interaction-a",
        &completed,
        1_100,
    );
    let projected = metrics_payload("interaction-a", 900, 1_000, 100);
    let metrics_batch = [("turn_metrics".to_owned(), projected.to_string())];
    let tail = append_batch(
        &store,
        &session_id,
        &host,
        "interaction-a",
        &metrics_batch,
        1_200,
    );

    /*
     * 日志里故意留一条水位之外的指标帧，但控制面事件水位不动：判定只看两个水位，
     * 不落后就不回源。回源一旦发生，这条记录会把轮次数顶到 2，用例立刻失败。
     */
    append_to_log_only(
        &store,
        &session_id,
        "interaction-b",
        &metrics_payload("interaction-b", 400, 500, 25),
        tail + 1,
    );
    mark_event_tail(&store, &session_id, tail);

    let merged = store
        .session_metrics_with_fallback(&session_id)
        .expect("读取累计指标")
        .expect("必须给出读数");
    assert_eq!(
        merged.turn_count, 1,
        "不落后时不得读日志，读数必须与投影一致"
    );
    assert_eq!(merged.total_input_tokens, 1_000);
    assert_eq!(merged.total_active_ms, 900);
}

/// 水位区间读不全时退回投影值：宁可用落后一点的读数，也不给偏小的总量。
#[test]
fn incomplete_read_falls_back_to_the_projection() {
    let (store, session_id, host) = fixture();
    let completed = [(
        "turn_completed".to_owned(),
        json!({ "interactionId": "interaction-a", "response": "结论" }).to_string(),
    )];
    append_batch(
        &store,
        &session_id,
        &host,
        "interaction-a",
        &completed,
        1_100,
    );
    let projected = metrics_payload("interaction-a", 900, 1_000, 100);
    let metrics_batch = [("turn_metrics".to_owned(), projected.to_string())];
    let tail = append_batch(
        &store,
        &session_id,
        &host,
        "interaction-a",
        &metrics_batch,
        1_200,
    );

    // 事件水位声称日志里有三条没投影的事件，实际一条也没有：读不全就不采纳。
    mark_event_tail(&store, &session_id, tail + 3);

    let merged = store
        .session_metrics_with_fallback(&session_id)
        .expect("读取累计指标")
        .expect("必须给出读数");
    assert_eq!(merged.turn_count, 1, "读不全时必须退回投影值");
    assert_eq!(merged.total_input_tokens, 1_000);
}
