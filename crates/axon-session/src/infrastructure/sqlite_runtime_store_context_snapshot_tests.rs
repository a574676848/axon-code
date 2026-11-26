//! 会话级上下文快照投影验收。
//!
//! 固定两件事：占用跟随真正带读数的用量帧、不带读数的流式帧不覆盖也不刷新时间戳；
//! 换窗生命周期帧不再产生任何 token 读数，因此不得改动快照。

use axon_domain::{DaemonSessionId, InteractionId, RuntimeHostId};
use axon_eventlog::{RuntimeEventKind, RuntimeEventRecord};
use serde_json::{Value, json};

use super::SqliteRuntimeStore;
use crate::domain::{ContextSnapshot, DaemonHostRecord};
use crate::ports::{EventLogStore, RuntimeStore};

const SESSION_ID: &str = "session-context-snapshot";
const INTERACTION_ID: &str = "interaction-context-snapshot";

fn fixture() -> (SqliteRuntimeStore, DaemonSessionId, DaemonHostRecord) {
    let directory = axon_test_support::test_temp_path("context-snapshot");
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    store.initialize().expect("初始化运行态库");
    let host = store
        .claim_host(
            &RuntimeHostId::new("host-context-snapshot").expect("Host ID"),
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

fn append(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    events: &[(String, String)],
    now_ms: u64,
) {
    let interaction_id = InteractionId::new(INTERACTION_ID).expect("Interaction ID");
    store
        .append_attributed_events(session_id, host, &interaction_id, events, now_ms)
        .expect("追加带归属的事件");
}

fn read(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> Option<ContextSnapshot> {
    store.context_snapshot(session_id).expect("读取上下文快照")
}

/// 一条真实的用量帧信封形状：`kind.type = usageUpdate`。
fn usage_frame(recorded_at_ms: i64, payload: Value) -> (String, String) {
    wire_frame(
        recorded_at_ms,
        json!({ "type": "usageUpdate", "payload": payload }),
    )
}

/// 一条轮转生命周期帧：与 CLI 进度投影下发的 wire 事实同形。
fn lifecycle_frame(recorded_at_ms: i64, kind: &str, summary: &str) -> (String, String) {
    wire_frame(
        recorded_at_ms,
        json!({
            "type": "turnLifecycle",
            "payload": {
                "partId": format!("lifecycle:compaction:inline:{SESSION_ID}"),
                "turnId": kind,
                "phase": "regularTurn",
                "status": "completed",
                "title": "窗口轮转",
                "kind": kind,
                "uiPhase": "history",
                "summary": summary,
                "metadata": {},
            },
        }),
    )
}

fn wire_frame(recorded_at_ms: i64, kind: Value) -> (String, String) {
    (
        "wire_event".to_owned(),
        json!({
            "schemaVersion": 1,
            "eventId": format!("{INTERACTION_ID}:{recorded_at_ms}"),
            "correlationId": INTERACTION_ID,
            "session": { "sessionId": SESSION_ID },
            "sequence": 0,
            "recordedAtMs": recorded_at_ms,
            "interactionId": INTERACTION_ID,
            "kind": kind,
        })
        .to_string(),
    )
}

#[test]
fn context_snapshot_follows_usage_frames() {
    let (store, session_id, host) = fixture();
    assert_eq!(
        read(&store, &session_id),
        None,
        "还没有任何读数时必须返回没有记录，而不是一个全空的结构体"
    );

    // 一帧真正带 window 占用的用量帧：快照必须跟随，时间取帧自己的记录时刻。
    let first = [usage_frame(
        100,
        json!({
            "inputTokens": 1_200,
            "outputTokens": 300,
            "currentContextTokens": 135_917,
            "contextWindow": 262_144,
            "systemPromptTokens": 9_000,
            "toolDefinitionTokens": 1_500,
            "conversationTokens": 80_821,
        }),
    )];
    append(&store, &session_id, &host, &first, 1_100);
    let observed = read(&store, &session_id).expect("首帧必须写出快照");
    assert_eq!(observed.current_context_tokens, Some(135_917));
    assert_eq!(observed.context_window, Some(262_144));
    assert_eq!(
        observed.composition,
        Some(json!({
            "systemPromptTokens": 9_000,
            "toolDefinitionTokens": 1_500,
            "conversationTokens": 80_821,
        })),
        "构成来自同一帧"
    );
    assert_eq!(observed.updated_at_ms, Some(100), "时间取帧记录时刻");
    assert_eq!(observed.source.as_deref(), Some("usage_frame"));

    // 只报增量、不带窗口占用的流式帧是常态：它不得覆盖读数，也不得刷新时间戳，
    // 否则界面会把旧数字当成「刚刚更新」。
    let streaming = [usage_frame(
        200,
        json!({ "inputTokens": 24_000, "outputTokens": 420 }),
    )];
    append(&store, &session_id, &host, &streaming, 1_200);
    assert_eq!(
        read(&store, &session_id),
        Some(observed.clone()),
        "不带占用的帧必须整条保持上一份真实快照（含时间戳）"
    );

    // 下一帧真实读数必须覆盖旧的：既不倒退，也不粘住上一份。
    let next = [usage_frame(300, json!({ "currentContextTokens": 140_000 }))];
    append(&store, &session_id, &host, &next, 1_300);
    let updated = read(&store, &session_id).expect("快照必须存在");
    assert_eq!(updated.current_context_tokens, Some(140_000));
    assert_eq!(updated.updated_at_ms, Some(300));
    assert_eq!(
        updated.composition, None,
        "新读数没带构成时必须显示未知，不得沿用上一份"
    );
    assert_eq!(updated.context_window, None, "窗口同理，不沿用旧值");
}

#[test]
fn late_frames_update_readings_without_rewinding_the_timestamp() {
    let (store, session_id, host) = fixture();
    let fresh = [usage_frame(
        1_000,
        json!({ "currentContextTokens": 90_000 }),
    )];
    append(&store, &session_id, &host, &fresh, 1_100);
    assert_eq!(
        read(&store, &session_id).and_then(|snapshot| snapshot.updated_at_ms),
        Some(1_000)
    );

    // 迟到的旧帧仍然更新读数，但「更新于」不得被拉回过去。
    let stale = [usage_frame(400, json!({ "currentContextTokens": 70_000 }))];
    append(&store, &session_id, &host, &stale, 1_200);
    let snapshot = read(&store, &session_id).expect("快照必须存在");
    assert_eq!(snapshot.current_context_tokens, Some(70_000));
    assert_eq!(
        snapshot.updated_at_ms,
        Some(1_000),
        "时间戳只前进不后退，否则界面会把旧读数标成更新"
    );
}

#[test]
fn rollover_lifecycle_frames_no_longer_write_token_readings() {
    let (store, session_id, host) = fixture();
    let usage = [usage_frame(
        100,
        json!({ "currentContextTokens": 135_917, "contextWindow": 262_144 }),
    )];
    append(&store, &session_id, &host, &usage, 1_100);
    let before = read(&store, &session_id).expect("用量帧必须写出快照");

    // 换窗完成帧不再携带任何 token 读数：不得改动占用、窗口或时间戳。
    let completed = [lifecycle_frame(
        200,
        "CompactionCompleted",
        "内联窗口轮转完成 · 消息 237 → 20 条 · 耗时 2761ms",
    )];
    append(&store, &session_id, &host, &completed, 1_200);
    assert_eq!(
        read(&store, &session_id),
        Some(before),
        "换窗帧不再产生读数，快照必须原样保持"
    );
}

/// 控制面会话的事件水位。
fn event_tail(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> u64 {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT last_event_sequence FROM sessions WHERE session_id=?1",
                [session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
        })
        .expect("读事件水位")
        .try_into()
        .expect("水位非负")
}

/// 只写事件日志、不投影：模拟「写路径已返回，投影 worker 还没消费这一批」。
fn append_to_log_only(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    sequence: u64,
    observed_at_ms: i64,
    current_context_tokens: u64,
) {
    let log = store.event_log(session_id);
    let record = RuntimeEventRecord::new(
        sequence,
        u64::try_from(observed_at_ms).expect("时刻非负"),
        RuntimeEventKind::Other("wire_event".to_owned()),
        json!({
            "recordedAtMs": observed_at_ms,
            "interactionId": INTERACTION_ID,
            "kind": {
                "type": "usageUpdate",
                "payload": { "currentContextTokens": current_context_tokens },
            },
        }),
    )
    .with_interaction(INTERACTION_ID);
    axon_eventlog::append_once(
        &log,
        sequence,
        u64::try_from(observed_at_ms).expect("时刻非负"),
        &[record],
    )
    .expect("直接写日志");
}

/// 把控制面事件水位推到指定序号。
fn mark_event_tail(store: &SqliteRuntimeStore, session_id: &DaemonSessionId, tail: u64) {
    store
        .with_read_connection(|connection| {
            connection.execute(
                "UPDATE sessions SET last_event_sequence=?1 WHERE session_id=?2",
                rusqlite::params![
                    i64::try_from(tail).expect("水位在范围内"),
                    session_id.as_str()
                ],
            )
        })
        .expect("推事件水位");
}

/// 投影表里当前落着的占用读数。
fn projected_tokens(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> Option<i64> {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT current_context_tokens FROM context_snapshots WHERE session_id=?1",
                [session_id.as_str()],
                |row| row.get::<_, Option<i64>>(0),
            )
        })
        .ok()
        .flatten()
}

/// 投影落后时读日志里的最新读数，且不把回源结果写回投影表。
#[test]
fn lagging_projection_reads_the_newest_log_reading() {
    let (store, session_id, host) = fixture();
    let projected = [usage_frame(
        100,
        json!({ "currentContextTokens": 135_917, "contextWindow": 262_144 }),
    )];
    append(&store, &session_id, &host, &projected, 1_100);
    let tail = event_tail(&store, &session_id);
    assert_eq!(tail, 1, "一帧一批：批首序号与事件尾都是 1");

    /*
     * 第二条用量帧只落事件日志：控制面水位已经推进，投影水位仍停在上一批，这正是
     * 写返回后立刻读投影的时间窗。
     */
    append_to_log_only(&store, &session_id, tail + 1, 200, 140_000);
    mark_event_tail(&store, &session_id, tail + 1);

    let observed = read(&store, &session_id).expect("快照必须存在");
    assert_eq!(
        observed.current_context_tokens,
        Some(140_000),
        "投影落后时必须回源到日志里更新的那条读数"
    );
    assert_eq!(
        observed.updated_at_ms,
        Some(200),
        "时间戳取自回源读到的那一帧"
    );
    assert_eq!(
        observed.context_window, None,
        "这一帧没带窗口，窗口就是未知"
    );
    assert_eq!(
        projected_tokens(&store, &session_id),
        Some(135_917),
        "回源结果不得写回投影表"
    );
}

/// 投影不落后时一次事件日志都不读，结果与投影完全一致。
#[test]
fn caught_up_projection_never_reads_the_log() {
    let (store, session_id, host) = fixture();
    let latest = [usage_frame(100, json!({ "currentContextTokens": 135_917 }))];
    append(&store, &session_id, &host, &latest, 1_100);
    let tail = event_tail(&store, &session_id);

    /*
     * 日志里故意再写一帧更晚的读数，但事件水位不动：判定只看两个水位，不落后就不回源。
     * 回源一旦发生，读数会被这帧顶成 140_000，用例立刻失败。
     */
    append_to_log_only(&store, &session_id, tail + 1, 200, 140_000);
    // 把投影行改成一个与日志都不同的值：读数只能来自投影，不可能来自日志。
    store
        .with_read_connection(|connection| {
            connection.execute(
                "UPDATE context_snapshots SET current_context_tokens = 111_111 \
                  WHERE session_id = ?1",
                [session_id.as_str()],
            )
        })
        .expect("改写投影行");

    let observed = read(&store, &session_id).expect("快照必须存在");
    assert_eq!(
        observed.current_context_tokens,
        Some(111_111),
        "不落后时必须原样返回投影值"
    );
    assert_eq!(
        observed.updated_at_ms,
        Some(100),
        "时间戳同样是投影里的那一份"
    );
}
