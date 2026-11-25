//! 常驻写句柄的回归用例。
//!
//! 三条必须成立的口径：连续追加复用同一句柄且序号连续、外部追加会被长度核对发现、
//! 日志整体重写后常驻句柄必须失效。注册表是进程级的，因此本组用例串行执行。

use std::sync::{Mutex, MutexGuard, PoisonError};

use tempfile::tempdir;

use crate::{EventLog, EventLogError, RuntimeEventKind, RuntimeEventRecord};

/// 进程级注册表是共享状态，用例之间必须串行，否则计数断言会互相看到对方的句柄。
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn record(sequence: u64) -> RuntimeEventRecord {
    RuntimeEventRecord::new(
        sequence,
        1_000 + sequence,
        RuntimeEventKind::Other("wire_event".to_owned()),
        serde_json::json!({ "interactionId": "interaction-a", "text": "x" }),
    )
}

fn sequences(log: &EventLog) -> Vec<u64> {
    let reader = log.reader().expect("读句柄");
    reader
        .records()
        .iter()
        .map(|record| record.sequence)
        .collect()
}

#[test]
fn repeated_appends_reuse_one_writer_and_keep_sequences() {
    let _guard = serial();
    let directory = tempdir().expect("临时目录");
    let log = EventLog::open(directory.path(), "session-reuse");
    let path = log.path();
    for sequence in 0..100_u64 {
        crate::append_once(&log, sequence, 1_000, &[record(sequence)]).expect("追加");
    }
    assert!(
        super::has_resident_writer(&path),
        "同一会话连续追加必须复用同一常驻句柄"
    );
    assert_eq!(
        sequences(&log),
        (0..100).collect::<Vec<_>>(),
        "复用句柄不得改变序号连续性"
    );
}

#[test]
fn an_external_append_is_detected_by_the_length_check() {
    let _guard = serial();
    let directory = tempdir().expect("临时目录");
    let log = EventLog::open(directory.path(), "session-external");
    crate::append_once(&log, 0, 1_000, &[record(0)]).expect("首批");

    // 另一个句柄直接写，模拟别的进程在本进程常驻句柄之外追加了一帧。
    let mut other = log.writer().expect("外部写句柄");
    other.append_batch(&[record(1)]).expect("外部追加");
    drop(other);

    // 常驻句柄记的长度已经过期：它必须先与磁盘对齐，再按尾号如实拒绝过期的序号。
    let error = crate::append_once(&log, 1, 1_000, &[record(1)]).expect_err("序号已过期");
    assert!(
        matches!(error, EventLogError::SequenceTailMismatch { .. }),
        "实际错误：{error:?}"
    );

    // 按对齐后的尾号续写应当成功，并且读回完整。
    crate::append_once(&log, 2, 1_000, &[record(2)]).expect("对齐后续写");
    assert_eq!(sequences(&log), vec![0, 1, 2]);
}

#[test]
fn a_rewrite_invalidates_the_resident_writer() {
    let _guard = serial();
    let directory = tempdir().expect("临时目录");
    let log = EventLog::open(directory.path(), "session-rewrite-resident");
    let path = log.path();
    crate::append_once(&log, 0, 1_000, &[record(0)]).expect("首批");
    assert!(super::has_resident_writer(&path));

    let header = crate::read_header(&path).expect("读头部").expect("有头部");
    crate::rewrite_log(&log, &header, &[record(0)]).expect("重写");

    assert!(
        !super::has_resident_writer(&path),
        "日志被整体替换后常驻句柄必须失效"
    );
    crate::append_once(&log, 1, 1_000, &[record(1)]).expect("重写后续写");
    assert_eq!(sequences(&log), vec![0, 1]);
}

#[test]
fn a_stale_generation_is_rejected_before_any_byte_is_written() {
    let _guard = serial();
    let directory = tempdir().expect("临时目录");
    let log = EventLog::open(directory.path(), "session-stale-generation");
    let path = log.path();
    crate::append_once(&log, 0, 1_000, &[record(0)]).expect("首批");

    // 旧代际的独立句柄：模拟重写之前就被在途追加取走的那个句柄。
    let stale_generation = super::current_generation(&path);
    let mut stale =
        crate::EventLogWriter::open_reusable(&log, 1, 1_000, stale_generation).expect("旧代际句柄");

    // 重写日志：注册表推进代际，旧句柄从此指向被替换的文件。
    let header = crate::read_header(&path).expect("读头部").expect("有头部");
    crate::rewrite_log(&log, &header, &[record(0)]).expect("重写");

    let error = stale
        .append_batch_reusing(1, super::current_generation(&path), &[record(1)])
        .expect_err("旧代际句柄不得写入");
    assert!(
        matches!(error, EventLogError::StaleHandle),
        "实际错误：{error:?}"
    );

    // 用当前代际重新打开的句柄可以正常续写。
    let current = super::current_generation(&path);
    let mut fresh =
        crate::EventLogWriter::open_reusable(&log, 1, 1_000, current).expect("当前代际句柄");
    fresh
        .append_batch_reusing(1, current, &[record(1)])
        .expect("当前代际可写");
    assert_eq!(sequences(&log), vec![0, 1]);
}
