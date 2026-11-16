//! 崩溃注入契约：任何时刻断电，日志都只能是「完整前缀 + 撕裂尾」。
//!
//! 用例只做真实文件操作，不替换 IO 实现：先写出正常日志，再按崩溃可能留下的三种形态
//! （半帧、半条记录、半帧头）直接改文件，然后验证读侧只采纳完整记录、写侧在重新
//! 打开时截断撕裂尾并继续追加。

use std::io::Write as _;

use axon_eventlog::{EventLog, EventLogError, RuntimeEventKind, RuntimeEventRecord, read_after};
use serde_json::json;

fn record(sequence: u64, event_type: RuntimeEventKind) -> RuntimeEventRecord {
    RuntimeEventRecord::new(
        sequence,
        1_700_000_000_000 + sequence,
        event_type,
        json!({"n": sequence}),
    )
}

/// 写出 `count` 条事件（每条一批），返回日志句柄。
fn seed_log(dir: &std::path::Path, session: &str, count: u64) -> EventLog {
    let log = EventLog::open(dir, session);
    let mut writer = log.writer().expect("写句柄");
    for sequence in 0..count {
        writer
            .append_batch(&[record(sequence, RuntimeEventKind::ToolCall)])
            .expect("追加");
    }
    drop(writer);
    log
}

fn append_bytes(log: &EventLog, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log.path())
        .expect("打开日志");
    file.write_all(bytes).expect("追加字节");
}

#[test]
fn crash_mid_frame_keeps_the_complete_prefix_and_recovers_on_reopen() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = seed_log(dir.path(), "session-crash-half-frame", 3);
    // 崩溃形态一：新帧只写了一半（魔数 + 截断的帧头）。
    append_bytes(&log, &[0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x00]);

    let reader = log.reader().expect("完整前缀必须可读");
    assert_eq!(reader.records().len(), 3, "只采纳完整记录");
    assert_eq!(reader.last_sequence(), Some(2));

    let mut writer = log.writer().expect("重新打开必须修复撕裂尾");
    assert_eq!(writer.next_sequence(), 3, "序号从已落盘尾部继续");
    writer
        .append_batch(&[record(3, RuntimeEventKind::TurnCompleted)])
        .expect("修复后应可续写");
    drop(writer);

    let reader = log.reader().expect("修复后可读");
    assert_eq!(reader.records().len(), 4);
    assert_eq!(
        read_after(&log.path(), 0, 10).expect("分页读取").len(),
        3,
        "撕裂尾不得在分页读取中出现"
    );
}

#[test]
fn crash_between_batches_loses_at_most_the_last_batch() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = seed_log(dir.path(), "session-crash-between", 2);
    let complete_len = std::fs::metadata(log.path()).expect("元信息").len();
    let mut writer = log.writer().expect("写句柄");
    writer
        .append_batch(&[record(2, RuntimeEventKind::ToolCall)])
        .expect("第三批");
    drop(writer);
    // 崩溃形态二：最后一批只落了一部分字节，然后进程消失。
    let handle = std::fs::OpenOptions::new()
        .write(true)
        .open(log.path())
        .expect("打开日志");
    handle
        .set_len(complete_len + 3)
        .expect("截断到最后一批中间");
    drop(handle);

    let reader = log.reader().expect("完整前缀必须可读");
    assert_eq!(reader.records().len(), 2, "只保留崩溃前的完整批次");
    let mut writer = log.writer().expect("重新打开");
    assert_eq!(writer.next_sequence(), 2);
    writer
        .append_batch(&[record(2, RuntimeEventKind::TurnCompleted)])
        .expect("续写被丢掉的序号");
    drop(writer);
    assert_eq!(log.reader().expect("可读").records().len(), 3);
}

#[test]
fn crash_before_header_is_written_leaves_no_half_state() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-crash-header");
    // 崩溃形态三：文件已创建但连 header 帧都没写完整。
    if let Some(parent) = log.path().parent() {
        std::fs::create_dir_all(parent).expect("建目录");
    }
    std::fs::write(log.path(), [0x28u8, 0xB5]).expect("写半帧头");

    // 读侧必须明确拒绝，而不是返回半条记录。
    let error = log.reader().expect_err("残缺 header 必须报错");
    assert!(
        matches!(error, EventLogError::Corrupt(_) | EventLogError::Io(_)),
        "实际错误：{error:?}"
    );

    // 写侧把不可解释的尾部整段截掉，重新建立 header。
    let mut writer = log.writer().expect("打开写句柄");
    assert_eq!(writer.next_sequence(), 0, "没有可继承的序号");
    writer
        .append_batch(&[record(0, RuntimeEventKind::TurnStarted)])
        .expect("重新建立日志");
    drop(writer);
    let reader = log.reader().expect("重建后可读");
    assert_eq!(reader.records().len(), 1);
}

#[test]
fn concurrent_writers_to_one_log_never_interleave_frames() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-concurrent");
    // 两个线程各自持有写句柄，靠文件锁串行：锁粒度是整个日志，因此帧不会交错。
    // 序号由写句柄按已落盘尾部给出，调用方不预设序号。
    let mut handles = Vec::new();
    for _ in 0..2_u8 {
        let log = log.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..5_u8 {
                let mut writer = log.writer().expect("抢到写句柄");
                let sequence = writer.next_sequence();
                writer
                    .append_batch(&[record(sequence, RuntimeEventKind::ToolCall)])
                    .expect("追加");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("线程应正常结束");
    }

    let reader = log.reader().expect("并发写入后必须可读");
    let sequences = reader
        .records()
        .iter()
        .map(|record| record.sequence)
        .collect::<Vec<_>>();
    assert_eq!(sequences.len(), 10, "全部记录都要落盘");
    assert_eq!(
        sequences,
        (0..10).collect::<Vec<_>>(),
        "帧不得交错：序号必须整体单调递增"
    );
}
