//! 会话累计指标的投影回源验收。
//!
//! 四件事各一条用例：投影落后时按日志补齐水位之后的轮次；投影不落后时一次事件日志都不读；
//! 水位停在批首序号时，早已投影完的最后一批不得被重复累加；跨度超过单批上界时按批读完。
//! 另有两条守卫：读不全退回投影值，边车索引不可用时根本不回源。全部在临时运行态根里跑。

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

/// 只写事件日志、不投影：把一批记录原样落盘，序号必须首尾相接。
fn append_records_to_log(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    records: &[RuntimeEventRecord],
) {
    let log = store.event_log(session_id);
    let first = records.first().expect("至少一条记录");
    axon_eventlog::append_once(&log, first.sequence, first.emitted_at_ms, records)
        .expect("直接写日志");
}

/// 一条用量帧的载荷：`kind.type = usageUpdate`，形状与投影取同一口径。
fn usage_payload(recorded_at_ms: i64, current_context_tokens: u64) -> Value {
    json!({
        "recordedAtMs": recorded_at_ms,
        "interactionId": "interaction-snapshot",
        "kind": {
            "type": "usageUpdate",
            "payload": { "currentContextTokens": current_context_tokens },
        },
    })
}

/// 当前快照里的上下文占用。
fn snapshot_tokens(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> Option<u64> {
    store
        .context_snapshot(session_id)
        .expect("读取上下文快照")
        .and_then(|snapshot| snapshot.current_context_tokens)
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

/// 水位跨度超过单批上界时按批读完：累计必须覆盖整段未投影区间。
#[test]
fn gap_beyond_one_batch_is_read_in_chunks() {
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
     * 日志里堆 2 600 条未投影指标帧：跨度超过单批 2 048，但在 8 192 的上界之内，必须分批
     * 读完而不是放弃回源。归属取同一个交互，避免查证语句的占位符随轮次膨胀。
     */
    let pending = 2_600_u64;
    let mut sequence = tail;
    let mut remaining = pending;
    while remaining > 0 {
        let batch = remaining.min(600);
        let records: Vec<RuntimeEventRecord> = (0..batch)
            .map(|_| {
                sequence += 1;
                RuntimeEventRecord::new(
                    sequence,
                    2_000 + sequence,
                    RuntimeEventKind::TurnMetrics,
                    metrics_payload("interaction-bulk", 10, 10, 1),
                )
                .with_interaction("interaction-bulk")
            })
            .collect();
        append_records_to_log(&store, &session_id, &records);
        remaining -= batch;
    }
    mark_event_tail(&store, &session_id, sequence);

    let merged = store
        .session_metrics_with_fallback(&session_id)
        .expect("读取累计指标")
        .expect("必须给出读数");
    assert_eq!(
        merged.turn_count,
        1 + pending,
        "跨度超过单批上界时必须分批读完，而不是退回投影值"
    );
    assert_eq!(merged.total_input_tokens, 1_000 + pending * 10);
    assert_eq!(merged.total_output_tokens, 100 + pending);
    assert_eq!(merged.total_active_ms, 900 + pending * 10);
}

/// 边车索引不可用时跳过回源：不得为了补一个读数触发整文件解帧。
#[test]
fn missing_index_skips_the_log_fallback() {
    let (store, session_id, host) = fixture();
    // 投影里先落一份读数：100ms 时占用 135 917。
    let projected = [(
        "wire_event".to_owned(),
        usage_payload(100, 135_917).to_string(),
    )];
    append_batch(
        &store,
        &session_id,
        &host,
        "interaction-snapshot",
        &projected,
        1_100,
    );
    assert_eq!(snapshot_tokens(&store, &session_id), Some(135_917));

    // 日志里再落一条更新的读数：控制面水位已推进，投影水位仍停在上一批。
    let unprojected = RuntimeEventRecord::new(
        2,
        200,
        RuntimeEventKind::Other("wire_event".to_owned()),
        usage_payload(200, 140_000),
    )
    .with_interaction("interaction-snapshot");
    append_records_to_log(&store, &session_id, &[unprojected]);
    mark_event_tail(&store, &session_id, 2);

    // 索引还在时：回源必须读到日志里更新的那条读数。
    assert_eq!(
        snapshot_tokens(&store, &session_id),
        Some(140_000),
        "索引可用时必须回源"
    );

    // 删掉边车索引：此时尾部扫描会退化成整文件解帧，必须按投影值返回。
    let log = store.event_log(&session_id);
    let mut index_path = log.path().into_os_string();
    index_path.push(".idx");
    std::fs::remove_file(std::path::PathBuf::from(index_path)).expect("删除边车索引");

    assert_eq!(
        snapshot_tokens(&store, &session_id),
        Some(135_917),
        "索引不可用时不得回源，必须退回投影值"
    );
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
