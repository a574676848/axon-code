//! 会话级上下文快照投影验收。
//!
//! 固定三件事：占用跟随真正带读数的用量帧、不带读数的流式帧不覆盖也不刷新时间戳、
//! 换窗前后读数只认精确值（结构化字段或精确整数文本），压缩后的展示值一律按未知处理。

use axon_domain::{DaemonSessionId, InteractionId, RuntimeHostId};
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
fn lifecycle_frame(
    recorded_at_ms: i64,
    kind: &str,
    status: &str,
    summary: &str,
    metadata: Value,
) -> (String, String) {
    wire_frame(
        recorded_at_ms,
        json!({
            "type": "turnLifecycle",
            "payload": {
                "partId": format!("lifecycle:compaction:inline:{SESSION_ID}"),
                "turnId": kind,
                "phase": "regularTurn",
                "status": status,
                "title": "窗口轮转",
                "kind": kind,
                "uiPhase": "history",
                "summary": summary,
                "metadata": metadata,
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
fn context_snapshot_records_rollover_before_after() {
    let (store, session_id, host) = fixture();

    // 换窗开始：运行时审计事件里只有换窗前读数（现场 825338）。
    let started = [lifecycle_frame(
        400,
        "CompactionStarted",
        "running",
        "正在开启新的记忆窗口 · 已耗时 0ms",
        json!({ "beforeTokens": 825_338 }),
    )];
    append(&store, &session_id, &host, &started, 1_400);
    let opened = read(&store, &session_id).expect("换窗开始必须写出快照");
    assert_eq!(opened.rollover_before_tokens, Some(825_338));
    assert_eq!(
        opened.rollover_after_tokens, None,
        "换窗还没完成时，换窗后读数必须未知"
    );
    assert_eq!(opened.updated_at_ms, Some(400));
    assert_eq!(opened.source.as_deref(), Some("rollover_frame"));

    // 换窗完成：现场形状 825338 → 31733。
    let completed = [lifecycle_frame(
        500,
        "CompactionCompleted",
        "completed",
        "记忆窗口切换完成 · 耗时 312ms",
        json!({ "beforeTokens": 825_338, "afterTokens": 31_733 }),
    )];
    append(&store, &session_id, &host, &completed, 1_500);
    let done = read(&store, &session_id).expect("换窗完成必须写出快照");
    assert_eq!(
        (done.rollover_before_tokens, done.rollover_after_tokens),
        (Some(825_338), Some(31_733)),
        "换窗前后读数必须与现场形状一致"
    );
    assert_eq!(done.updated_at_ms, Some(500));

    // 换窗失败：没有新窗口，换窗后读数必须回到未知，不能把帧里的 0 读成「降到 0」。
    let failed = [lifecycle_frame(
        600,
        "CompactionFailed",
        "failed",
        "内联窗口轮转未完成：新窗口仍超线；当前任务历史保持不变",
        json!({ "beforeTokens": 840_000, "afterTokens": 0 }),
    )];
    append(&store, &session_id, &host, &failed, 1_600);
    let failed_snapshot = read(&store, &session_id).expect("换窗失败同样要留痕");
    assert_eq!(failed_snapshot.rollover_before_tokens, Some(840_000));
    assert_eq!(failed_snapshot.rollover_after_tokens, None);
}

#[test]
fn context_snapshot_reads_exact_numbers_and_leaves_compacted_text_unknown() {
    let (store, session_id, host) = fixture();

    // 帧里只有展示文本时，精确整数仍然可以读出来（运行时自己的进度行形状）。
    let exact = [lifecycle_frame(
        700,
        "CompactionCompleted",
        "completed",
        "内联窗口轮转完成 · 消息 42 → 6 条 · tokens 825338 → 31733 · 耗时 312ms",
        json!({}),
    )];
    append(&store, &session_id, &host, &exact, 1_700);
    let parsed = read(&store, &session_id).expect("快照必须存在");
    assert_eq!(
        (parsed.rollover_before_tokens, parsed.rollover_after_tokens),
        (Some(825_338), Some(31_733)),
        "消息条数不得被误读成 token 读数"
    );

    // 同一句被 format_decimal_count 压成 825K / 31.7K 时不得反推近似值：
    // 宁可显示未知，也不能让界面拿一个猜出来的数字冒充读数。
    let compacted = [lifecycle_frame(
        800,
        "CompactionCompleted",
        "completed",
        "内联窗口轮转完成 · 消息 42 → 6 条 · tokens 825K → 31.7K · 耗时 312ms",
        json!({}),
    )];
    append(&store, &session_id, &host, &compacted, 1_800);
    let unknown = read(&store, &session_id).expect("快照必须存在");
    assert_eq!(
        (
            unknown.rollover_before_tokens,
            unknown.rollover_after_tokens
        ),
        (None, None),
        "压缩后的展示值只能显示未知"
    );
    assert_eq!(
        unknown.updated_at_ms,
        Some(800),
        "换窗事实本身仍然要刷新时间戳"
    );
}
