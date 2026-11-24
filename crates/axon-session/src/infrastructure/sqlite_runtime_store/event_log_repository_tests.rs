//! 事件面写入的序号一致性用例。
use super::*;
use crate::ports::RuntimeStore as _;
use axon_domain::RuntimeHostId;

fn fixture() -> (SqliteRuntimeStore, DaemonSessionId, DaemonHostRecord) {
    let directory = axon_test_support::test_temp_path("event-log-repository");
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    store.initialize().expect("初始化");
    let host_id = RuntimeHostId::new("host-1").expect("host id");
    let host = store
        .claim_host(&host_id, 1, None, 10, 60_000)
        .expect("claim host");
    let session_id = DaemonSessionId::new("session-session-repository").expect("session id");
    store
        .create_session(&session_id, ".", 10)
        .expect("建立会话");
    (store, session_id, host)
}

/// 控制面水位（`sessions.last_event_sequence`）。
fn watermark(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> u64 {
    let connection = store.connection().expect("取连接");
    let value: i64 = connection
        .query_row(
            "SELECT last_event_sequence FROM sessions WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get(0),
        )
        .expect("读控制面水位");
    u64::try_from(value).expect("水位非负")
}

/// 控制面已经分配好的记录（`write_records_locked` 的入参形状）。
fn allocated(
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    sequence: u64,
) -> SessionEventRecord {
    SessionEventRecord {
        session_id: session_id.clone(),
        sequence,
        generation: host.generation.clone(),
        event_type: "checkpoint".to_owned(),
        payload_json: "{}".to_owned(),
        emitted_at_ms: 1_000 + sequence,
    }
}

/// 模拟控制面事务推进水位：`write_records_locked` / `write_events_at_locked` 的调用方负责这一步。
fn advance_watermark(store: &SqliteRuntimeStore, session_id: &DaemonSessionId, sequence: u64) {
    store
        .connection()
        .expect("取连接")
        .execute(
            "UPDATE sessions SET last_event_sequence=?1 WHERE session_id=?2",
            params![i64_value(sequence).expect("水位"), session_id.as_str()],
        )
        .expect("推进水位");
}

/// 越过控制面直接落一条事件：模拟另一写者已经写入、控制面水位还没跟上。
fn append_bypassing_watermark(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    sequence: u64,
) {
    let record = to_runtime_record(&allocated(session_id, host, sequence), None, None);
    axon_eventlog::append_once(&store.event_log(session_id), sequence, 1_000, &[record])
        .expect("绕过控制面落盘");
}

fn metrics_events() -> [(String, String); 1] {
    let metrics = axon_eventlog::TurnMetrics {
        turn_id: "turn-1".to_owned(),
        ..axon_eventlog::TurnMetrics::default()
    };
    [(
        "turn_metrics".to_owned(),
        serde_json::to_string(&metrics).expect("序列化指标"),
    )]
}

#[test]
fn watermark_rollback_then_retry_succeeds() {
    let (store, session_id, host) = fixture();
    let events = metrics_events();
    append_events(&store, &session_id, &host, &events, 20, None).expect("首批事件");
    assert_eq!(watermark(&store, &session_id), 1);

    // 另一写者已落盘第 2 条，控制面水位仍停在 1：下一次按水位领到 2。
    // 存储层补偿水位并按文件尾重排后重试，事件不得丢。
    append_bypassing_watermark(&store, &session_id, &host, 2);
    let records =
        append_events(&store, &session_id, &host, &events, 30, None).expect("补偿后重试必须成功");
    assert_eq!(records[0].sequence, 3, "重排后落盘的是文件尾之后的号");
    assert_eq!(watermark(&store, &session_id), 3, "水位必须跟着重排后的号");

    // 落帧入口走同一条补偿出口：调用方先推进水位，再持会话锁落帧。
    append_bypassing_watermark(&store, &session_id, &host, 4);
    advance_watermark(&store, &session_id, 4);
    let lock = store.event_log_write_lock(&session_id);
    let guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    write_records_locked(&store, &mut [allocated(&session_id, &host, 4)], None)
        .expect("写记录路径同样必须补偿后落盘");
    drop(guard);

    let stored = replay_events(&store, &session_id, 0, 10).expect("回放");
    assert_eq!(
        stored
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5],
        "补偿重试不得丢事件，序号必须严格递增且无洞"
    );
    assert_eq!(watermark(&store, &session_id), 5, "水位必须等于落盘末号");
}

#[test]
fn compensation_never_rolls_back_a_later_writers_watermark() {
    let (store, session_id, host) = fixture();
    append_events(&store, &session_id, &host, &metrics_events(), 20, None).expect("首批事件");

    // 更晚的写者已经领走 9：水位停在 9，文件尾仍是 1。
    advance_watermark(&store, &session_id, 9);
    // 本次写入推进到的是 2，CAS 比对不上：不得把水位硬拉回文件尾。
    write_failure::compensate_watermark(&store, &session_id, 2);
    assert_eq!(
        watermark(&store, &session_id),
        9,
        "不得抹掉更晚写者领走的号"
    );

    // 水位仍等于本次写入的值时才回退到落盘尾。
    write_failure::compensate_watermark(&store, &session_id, 9);
    assert_eq!(watermark(&store, &session_id), 1, "回退目标是落盘尾");
}

#[test]
fn compensation_then_realign_recovers_a_writer_left_behind() {
    let (store, session_id, host) = fixture();
    let log = store.event_log(&session_id);
    // 先把日志尾做成 56。
    let prefix = (1..=56u64)
        .map(|sequence| to_runtime_record(&allocated(&session_id, &host, sequence), None, None))
        .collect::<Vec<_>>();
    axon_eventlog::append_once(&log, 1, 1_000, &prefix).expect("前置 1..=56");

    // A 领到 57、B 领到 58，都还没落盘。
    advance_watermark(&store, &session_id, 57);
    advance_watermark(&store, &session_id, 58);
    // B 先拿到文件锁：expected 58 对不上尾 56，补偿把水位写回 56（A 的 57 一起被抹掉）。
    write_failure::compensate_watermark(&store, &session_id, 58);
    assert_eq!(watermark(&store, &session_id), 56);

    // A 随后以尾 56+1 落盘 57：此刻水位 56 落后于落盘尾 57。
    let a_record = to_runtime_record(&allocated(&session_id, &host, 57), None, None);
    axon_eventlog::append_once(&log, 57, 1_000, &[a_record]).expect("A 落盘 57");
    assert_eq!(
        watermark(&store, &session_id),
        56,
        "前提：水位必须落后于落盘尾"
    );
    assert_eq!(
        durable_tail(&store, &session_id)
            .expect("读落盘尾")
            .map(|(sequence, _)| sequence),
        Some(57),
        "前提：落盘尾已经越过水位"
    );

    // B 的重排：水位落后于落盘尾是「可纠正的缓存」，必须允许前推到尾后一段。
    // 旧实现要求水位恰好等于落盘尾，这里会 0 行失败，B 的事件只能丢。
    let mut b_records = vec![to_runtime_record(
        &allocated(&session_id, &host, 58),
        None,
        None,
    )];
    let claimed = write_failure::realign_sequences(&store, &session_id, &mut b_records)
        .expect("水位落后于落盘尾时必须允许重排");
    assert_eq!(claimed, 58, "重排后的水位必须等于本批末号");
    assert_eq!(b_records[0].sequence, 58, "B 的事件必须落在文件尾之后");
    assert_eq!(
        watermark(&store, &session_id),
        58,
        "水位必须被前推到落盘末号"
    );

    // 重排之后落盘必须成功：并发失败方的事件不能被丢掉。
    write_failure::append_preallocated(&store, &session_id, 1_000, &mut b_records, claimed)
        .expect("重排后 B 的事件必须落盘");

    let stored = replay_events(&store, &session_id, 0, 100).expect("回放");
    assert_eq!(
        stored
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>(),
        (1..=58).collect::<Vec<_>>(),
        "A 与 B 的事件都必须落盘，序号连续且无洞"
    );
}

/// 同一条会话的并发写入必须仍然拿到连续序号。
///
/// 序号分配在控制面事务里、文件追加在事务提交之后：两步之间没有互斥时，两个写者
/// 可以按相反顺序落盘，后落盘的一方撞上「序号与文件尾不一致」，只能回退水位重排。
/// 重排会改掉已经发布的序号，订阅端把这一批当重复帧丢掉，界面上就出现「服务端已
/// 收口、界面还在跑」。会话锁把分配与落盘串起来之后，并发下序号必须仍然连续无洞。
#[test]
fn concurrent_appends_keep_the_sequence_contiguous() {
    let (store, session_id, host) = fixture();
    let store = std::sync::Arc::new(store);
    let workers = 8usize;
    let per_worker = 16u64;
    let mut handles = Vec::new();
    for worker in 0..workers {
        let store = std::sync::Arc::clone(&store);
        let session_id = session_id.clone();
        let host = host.clone();
        handles.push(std::thread::spawn(move || {
            for index in 0..per_worker {
                let payload = format!("{{\"worker\":{worker},\"index\":{index}}}");
                let events = vec![("checkpoint".to_owned(), payload)];
                append_events(&store, &session_id, &host, &events, 1_000, None)
                    .expect("并发追加必须成功");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("并发追加线程异常退出");
    }

    let total = (workers as u64) * per_worker;
    let stored =
        replay_events(&store, &session_id, 0, u32::try_from(total).expect("上限")).expect("回放");
    assert_eq!(
        stored
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>(),
        (1..=total).collect::<Vec<_>>(),
        "并发写入不得丢事件、不得重复、不得留洞"
    );
    assert_eq!(
        watermark(&store, &session_id),
        total,
        "控制面水位必须等于落盘末号"
    );
}
