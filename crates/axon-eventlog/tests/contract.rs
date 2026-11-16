//! 事件日志的对外契约测试。

use std::io::Write as _;

use axon_eventlog::{
    EventLog, EventLogError, JobOutcome, JobRef, RuntimeEventKind, RuntimeEventRecord, TurnMetrics,
    TurnOrigin,
};
use serde_json::json;

fn record(
    sequence: u64,
    event_type: RuntimeEventKind,
    payload: serde_json::Value,
) -> RuntimeEventRecord {
    RuntimeEventRecord::new(sequence, 1_700_000_000_000 + sequence, event_type, payload)
}

#[test]
fn append_then_read_round_trips() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-a");

    let mut writer = log.writer().expect("应可打开写句柄");
    assert_eq!(writer.next_sequence(), 0);
    let last = writer
        .append_batch(&[
            record(0, RuntimeEventKind::TurnStarted, json!({"turnId": "t1"})),
            record(1, RuntimeEventKind::ToolCall, json!({"name": "read"})),
        ])
        .expect("应可追加");
    assert_eq!(last, 1);
    assert_eq!(writer.next_sequence(), 2);
    drop(writer);

    let reader = log.reader().expect("应可读取");
    assert_eq!(reader.header().session_id, "session-session-a");
    assert_eq!(reader.records().len(), 2);
    assert_eq!(reader.last_sequence(), Some(1));
    // read_from 的语义是"序号严格大于 after"，与网关重放游标一致
    assert_eq!(reader.read_from(0, 10).len(), 1);
    assert!(reader.read_from(1, 10).is_empty());
    assert_eq!(reader.read_tail(10).len(), 2);
    assert_eq!(reader.read_tail(1)[0].sequence, 1);
}

#[test]
fn sequence_gap_is_rejected_without_writing() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-b");
    let mut writer = log.writer().expect("应可打开写句柄");

    let error = writer
        .append_batch(&[record(5, RuntimeEventKind::TurnStarted, json!({}))])
        .expect_err("序号跳号应被拒绝");
    assert!(matches!(
        error,
        EventLogError::SequenceGap {
            expected: 0,
            actual: 5
        }
    ));
    assert_eq!(writer.next_sequence(), 0, "被拒绝的批次不得推进序号");
    drop(writer);

    // 被拒绝的批次不得产生任何落盘产物：未写入时日志文件不应存在
    assert!(!log.path().exists(), "未成功追加时不应创建日志文件");
}

#[test]
fn empty_batch_is_rejected() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-c");
    let mut writer = log.writer().expect("应可打开写句柄");
    let error = writer.append_batch(&[]).expect_err("空批次应被拒绝");
    assert!(matches!(error, EventLogError::EmptyBatch));
}

#[test]
fn reopen_continues_from_stored_tail() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-d");

    {
        let mut writer = log.writer().expect("首次打开");
        writer
            .append_batch(&[record(0, RuntimeEventKind::TurnStarted, json!({}))])
            .expect("首批");
    }

    let mut writer = log.writer().expect("再次打开");
    assert_eq!(writer.next_sequence(), 1, "应从已提交尾部续写");
    writer
        .append_batch(&[record(1, RuntimeEventKind::TurnCompleted, json!({}))])
        .expect("续写");
    drop(writer);

    let reader = log.reader().expect("应可读取");
    assert_eq!(reader.records().len(), 2);
}

#[test]
fn torn_tail_is_truncated_before_next_append() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-e");

    {
        let mut writer = log.writer().expect("首次打开");
        writer
            .append_batch(&[record(0, RuntimeEventKind::TurnStarted, json!({}))])
            .expect("首批");
    }

    // 模拟崩溃：在文件末尾追加半帧垃圾
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log.path())
        .expect("打开日志");
    let half_frame = [0x28u8, 0xB5, 0x2F, 0xFD, 0x00, 0x00];
    file.write_all(&half_frame).expect("写入半帧");
    drop(file);

    let snapshot = log.reader().expect("撕裂尾仍可读");
    assert_eq!(snapshot.records().len(), 1, "完整前缀必须保留");
    assert_eq!(
        snapshot.header().session_id,
        "session-session-e",
        "header 必须可解析"
    );

    // 写句柄打开时应自动截断撕裂尾，并从已提交尾部继续
    let mut writer = log.writer().expect("打开写句柄");
    assert_eq!(writer.next_sequence(), 1);
    writer
        .append_batch(&[record(1, RuntimeEventKind::TurnCompleted, json!({}))])
        .expect("截断后应可续写");
    drop(writer);

    let reader = log.reader().expect("修复后可读");
    assert_eq!(reader.records().len(), 2);
}

#[test]
fn append_once_repairs_torn_tail_before_appending() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-g");

    {
        let mut writer = log.writer().expect("首次打开");
        writer
            .append_batch(&[record(0, RuntimeEventKind::TurnStarted, json!({}))])
            .expect("首批");
    }

    // 模拟崩溃：末尾留下半帧（魔数 + 截断的帧头）
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log.path())
        .expect("打开日志");
    file.write_all(&[0x28u8, 0xB5, 0x2F, 0xFD, 0x00, 0x00])
        .expect("写入半帧");
    drop(file);

    // 控制面续写走 append_once：必须先截断撕裂尾，再写新帧
    axon_eventlog::append_once(
        &log,
        1,
        1_700_000_000_000,
        &[record(1, RuntimeEventKind::TurnCompleted, json!({}))],
    )
    .expect("撕裂尾修复后应可追加");

    let reader = log.reader().expect("修复后可读");
    assert_eq!(reader.records().len(), 2, "撕裂尾之后追加的帧必须仍然可见");
    assert_eq!(reader.last_sequence(), Some(1));
}

/// 生成不可压缩的载荷：压缩后仍然大于尾部自检窗口，用来覆盖「窗口落在帧内部」的分支。
fn incompressible_payload(bytes: usize) -> String {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut out = String::with_capacity(bytes);
    for _ in 0..bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push(char::from(b'!' + u8::try_from(state % 90).unwrap_or(0)));
    }
    out
}

#[test]
fn append_once_keeps_healthy_tail_larger_than_tail_window() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-h");
    let blob = incompressible_payload(3 * 1024 * 1024);

    {
        let mut writer = log.writer().expect("首次打开");
        writer
            .append_batch(&[record(
                0,
                RuntimeEventKind::TurnStarted,
                json!({"blob": blob.clone()}),
            )])
            .expect("首批");
    }

    // 末帧远大于尾部自检窗口：自检必须在找不到帧边界时回退全量扫描，
    // 且不得把健康日志当成撕裂尾截断。
    axon_eventlog::append_once(
        &log,
        1,
        1_700_000_000_000,
        &[record(1, RuntimeEventKind::TurnCompleted, json!({}))],
    )
    .expect("健康日志应可续写");

    let reader = log.reader().expect("应可读取");
    assert_eq!(reader.records().len(), 2, "健康日志不得被截断");
    let stored = reader.records()[0]
        .payload
        .get("blob")
        .and_then(|value| value.as_str())
        .expect("载荷应保留");
    assert_eq!(stored.len(), blob.len(), "大载荷必须逐字节保留");
    assert_eq!(stored, blob);
}

#[test]
fn lazy_paged_reads_match_the_full_snapshot() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-paged");
    let mut writer = log.writer().expect("写句柄");
    let mut sequence = 0_u64;
    for batch in 0..20_u64 {
        let events = (0..5)
            .map(|_| {
                let record = record(
                    sequence,
                    RuntimeEventKind::ToolCall,
                    json!({"batch": batch}),
                );
                sequence += 1;
                record
            })
            .collect::<Vec<_>>();
        writer.append_batch(&events).expect("批次写入");
    }
    writer.append_batch(&[]).ok();
    drop(writer);

    let full = log.reader().expect("全量读取");
    for after in [0_u64, 3, 27, 99, 100] {
        for limit in [1_usize, 7, 100] {
            let expected = full.read_from(after, limit);
            let actual = axon_eventlog::read_after(&log.path(), after, limit).expect("分页读取");
            assert_eq!(
                actual, expected,
                "分页结果必须与全量快照一致：after={after} limit={limit}"
            );
        }
    }

    let matched = |record: &RuntimeEventRecord| record.sequence.is_multiple_of(3);
    let matched_all = full
        .records()
        .iter()
        .filter(|record| matched(record))
        .cloned()
        .collect::<Vec<_>>();
    for limit in [1_usize, 4, 50] {
        let start = matched_all.len().saturating_sub(limit);
        let expected = matched_all[start..].to_vec();
        let actual =
            axon_eventlog::read_tail_matching(&log.path(), limit, matched).expect("尾窗读取");
        assert_eq!(
            actual, expected,
            "尾窗结果必须与全量过滤一致：limit={limit}"
        );
    }
}

#[test]
#[ignore = "性能验收：2000 帧会话日志的分页读取只解压尾帧，门槛 30ms"]
fn lazy_paged_read_stays_bounded_on_long_logs() {
    const FRAMES: u64 = 2_000;
    const PER_FRAME: u64 = 10;
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-biglog");
    {
        let mut writer = log.writer().expect("写句柄");
        let mut sequence = 0_u64;
        for _ in 0..FRAMES {
            let events = (0..PER_FRAME)
                .map(|_| {
                    let record = record(
                        sequence,
                        RuntimeEventKind::ToolCall,
                        json!({"payload": "x"}),
                    );
                    sequence += 1;
                    record
                })
                .collect::<Vec<_>>();
            writer.append_batch(&events).expect("批次写入");
        }
    }

    let full_started = std::time::Instant::now();
    let full = log.reader().expect("全量读取");
    let full_ms = full_started.elapsed().as_millis();
    assert_eq!(
        full.records().len(),
        usize::try_from(FRAMES * PER_FRAME).expect("条数")
    );

    let mut best_ms = u128::MAX;
    let mut last_page = Vec::new();
    for _ in 0..5 {
        let started = std::time::Instant::now();
        last_page =
            axon_eventlog::read_after(&log.path(), FRAMES * PER_FRAME - 5, 5).expect("分页读取");
        best_ms = best_ms.min(started.elapsed().as_millis());
    }
    assert_eq!(last_page.len(), 4, "after 之后只剩 4 条");
    assert_eq!(last_page[3].sequence, FRAMES * PER_FRAME - 1);
    println!(
        "eventlog_paged frames={FRAMES} records={} page={best_ms}ms full={full_ms}ms 门槛=30ms",
        FRAMES * PER_FRAME
    );
    assert!(best_ms < 30, "分页读取超出 30ms 门槛：{best_ms}ms");
    assert!(
        best_ms < full_ms,
        "分页读取必须明显快于全量解压：page={best_ms}ms full={full_ms}ms"
    );
}

#[test]
fn non_monotonic_frames_are_rejected_across_frame_boundaries() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-monotonic");
    // 模拟「控制面水位倒退后继续写入」：第二帧的序号比第一帧小。
    axon_eventlog::append_once(
        &log,
        5,
        1_700_000_000_000,
        &[record(5, RuntimeEventKind::TurnStarted, json!({}))],
    )
    .expect("第一帧");
    axon_eventlog::append_once(
        &log,
        0,
        1_700_000_000_000,
        &[record(0, RuntimeEventKind::TurnStarted, json!({}))],
    )
    .expect("第二帧（序号倒退）");
    let error = log.reader().expect_err("跨帧序号倒退必须被拒绝");
    assert!(
        matches!(
            error,
            EventLogError::SequenceGap {
                expected: 6,
                actual: 0
            }
        ),
        "实际错误：{error:?}"
    );
}

#[test]
fn turn_metrics_keeps_origin_and_unmeasured_ttft() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-f");
    let mut writer = log.writer().expect("应可打开写句柄");

    let metrics = TurnMetrics {
        turn_id: "t1".to_owned(),
        ttft_ms: None,
        active_ms: 900,
        input_tokens: 10,
        output_tokens: 20,
        ..TurnMetrics::default()
    };
    let event = RuntimeEventRecord::turn_metrics(0, 1, &metrics)
        .expect("可构造")
        .with_interaction("interaction-1")
        .with_origin(Some("tui"), TurnOrigin::External);
    writer.append_batch(&[event]).expect("应可追加");
    drop(writer);

    let reader = log.reader().expect("应可读取");
    let stored = &reader.records()[0];
    assert_eq!(stored.origin_channel.as_deref(), Some("tui"));
    assert_eq!(stored.origin_kind, TurnOrigin::External);
    assert_eq!(stored.interaction_id.as_deref(), Some("interaction-1"));
    let parsed: TurnMetrics = serde_json::from_value(stored.payload.clone()).expect("载荷应可还原");
    assert!(parsed.ttft_ms.is_none(), "未测得首字必须保持 None");
    assert_eq!(parsed.active_ms, 900);
}
#[test]
fn job_events_round_trip() {
    let started = JobRef {
        job_id: "job-1".to_owned(),
        kind: "shell".to_owned(),
        owner: "session-session-a".to_owned(),
        started_at_ms: 100,
    };
    let event = RuntimeEventRecord::job_started(0, 100, &started).expect("可构造");
    let line = event.encode_line().expect("可编码");
    let decoded = RuntimeEventRecord::decode_line(&line).expect("可解码");
    assert_eq!(decoded.event_type, RuntimeEventKind::JobStarted);
    let parsed: JobRef = serde_json::from_value(decoded.payload).expect("载荷可还原");
    assert_eq!(parsed.job_id, "job-1");
    assert_eq!(parsed.kind, "shell");

    let outcome = JobOutcome {
        job_id: "job-1".to_owned(),
        status: "completed".to_owned(),
        elapsed_ms: Some(900),
        exit_code: Some(0),
        terminal_reason: None,
    };
    let event = RuntimeEventRecord::job_completed(1, 1_000, &outcome).expect("可构造");
    let decoded =
        RuntimeEventRecord::decode_line(&event.encode_line().expect("可编码")).expect("可解码");
    assert_eq!(decoded.event_type, RuntimeEventKind::JobCompleted);
    let parsed: JobOutcome = serde_json::from_value(decoded.payload).expect("载荷可还原");
    assert_eq!(parsed.elapsed_ms, Some(900));
    assert_eq!(parsed.exit_code, Some(0));
    assert!(parsed.terminal_reason.is_none());
}
