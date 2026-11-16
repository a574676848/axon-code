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
    let log = EventLog::open(dir.path(), "session-lane-a");

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
    assert_eq!(reader.header().session_id, "session-lane-a");
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
    let log = EventLog::open(dir.path(), "session-lane-b");
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
    let log = EventLog::open(dir.path(), "session-lane-c");
    let mut writer = log.writer().expect("应可打开写句柄");
    let error = writer.append_batch(&[]).expect_err("空批次应被拒绝");
    assert!(matches!(error, EventLogError::EmptyBatch));
}

#[test]
fn reopen_continues_from_stored_tail() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-lane-d");

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
    let log = EventLog::open(dir.path(), "session-lane-e");

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
        "session-lane-e",
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
fn turn_metrics_keeps_origin_and_unmeasured_ttft() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-lane-f");
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
        owner: "session-lane-a".to_owned(),
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
