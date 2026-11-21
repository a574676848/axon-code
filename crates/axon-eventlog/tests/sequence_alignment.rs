//! 序号与文件尾对齐（INV-1）的验收测试。
//!
//! 覆盖三件事：写入前必须核对文件尾、空日志只接受首个序号 1、并发写者补偿后不丢事件。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Barrier, Mutex};

use axon_eventlog::{
    EventLog, EventLogError, RuntimeEventKind, RuntimeEventRecord, read_tail_record,
};
use serde_json::json;

/// 单个并发写者追加的条数。
const PER_WRITER: usize = 200;
/// 并发写者数。
const WRITERS: usize = 2;
/// 单条记录的追加尝试上限：含序号冲突后的重排重试与抢锁超时重试，正常远达不到它。
const MAX_ATTEMPTS: u32 = 64;

fn record(sequence: u64, tag: &str) -> RuntimeEventRecord {
    RuntimeEventRecord::new(
        sequence,
        1_700_000_000_000 + sequence,
        RuntimeEventKind::Other("wire_event".to_owned()),
        json!({ "tag": tag }),
    )
}

/// 已落盘的最后一条记录序号。
fn tail_sequence(log: &EventLog) -> Option<u64> {
    read_tail_record(&log.path())
        .expect("读落盘尾")
        .map(|record| record.sequence)
}

#[test]
fn append_once_rejects_sequence_that_skips_file_tail() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-tail-check");
    let prefix = (1..=59).map(|seq| record(seq, "前置")).collect::<Vec<_>>();
    axon_eventlog::append_once(&log, 1, 1_000, &prefix).expect("前置帧");
    let before = std::fs::read(log.path()).expect("读取日志字节");
    assert_eq!(tail_sequence(&log), Some(59));

    // 文件尾 59、调用方声明 57：必须显式失败，而不是把第 57 条写在 59 之后。
    let error = axon_eventlog::append_once(&log, 57, 1_000, &[record(57, "逆序")])
        .expect_err("序号与文件尾不一致必须失败");
    match error {
        EventLogError::SequenceTailMismatch { expected, actual } => {
            assert_eq!((expected, actual), (57, 59), "必须原样带出声明序号与文件尾");
        }
        other => panic!("应返回 SequenceTailMismatch，实际：{other:?}"),
    }
    let after = std::fs::read(log.path()).expect("复读日志字节");
    assert_eq!(after, before, "被拒绝的写入不得改动文件任何一个字节");
    let snapshot = log.reader().expect("拒绝后仍可读");
    assert_eq!(snapshot.records().len(), 59, "不得多出第 57 条");
    assert_eq!(snapshot.last_sequence(), Some(59));
}

#[test]
fn empty_and_header_only_logs_accept_only_origin_sequences() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-empty");
    let error = axon_eventlog::append_once(&log, 2, 1_000, &[record(2, "跳号")])
        .expect_err("空日志不得从 2 起写");
    assert!(
        matches!(
            error,
            EventLogError::SequenceTailMismatch {
                expected: 2,
                actual: 0
            }
        ),
        "实际错误：{error:?}"
    );
    assert!(!log.path().exists(), "被拒绝的写入不得创建日志文件");

    // 事件日志层的原点是 0：先按 0 写一条，此时文件尾才出现 0，下一个号必须是 1。
    axon_eventlog::append_once(&log, 0, 1_000, &[record(0, "原点")]).expect("空日志允许从 0 起写");
    assert_eq!(tail_sequence(&log), Some(0));
    assert!(
        axon_eventlog::append_once(&log, 0, 1_000, &[record(0, "重复")]).is_err(),
        "有记录之后序号即位置，不得重复写 0"
    );
    axon_eventlog::append_once(&log, 1, 1_000, &[record(1, "第二条")]).expect("接在 0 之后");

    // 只有 header 帧的日志同样是空日志：header 不是记录，不能算成尾号。
    let header_only = EventLog::open(dir.path(), "session-session-header-only");
    let header = axon_eventlog::Header::new("session-session-header-only", 1_000, None);
    let frame = axon_eventlog::encode_frame(&header.encode_line().expect("header 行"))
        .expect("压缩 header 帧");
    std::fs::create_dir_all(header_only.path().parent().expect("会话目录")).expect("建会话目录");
    std::fs::write(header_only.path(), frame).expect("写 header 帧");
    let error = axon_eventlog::append_once(&header_only, 2, 1_000, &[record(2, "跳号")])
        .expect_err("只有 header 帧时不得从 2 起写");
    assert!(
        matches!(
            error,
            EventLogError::SequenceTailMismatch {
                expected: 2,
                actual: 0
            }
        ),
        "实际错误：{error:?}"
    );
    axon_eventlog::append_once(&header_only, 1, 1_000, &[record(1, "首条")])
        .expect("header 帧不算记录，控制面首次分配 1 必须可写");
}

/// 控制面水位的测试模型：先领号、再落盘，落盘失败时按文件尾重算。
struct Watermark {
    next: Mutex<u64>,
}

impl Watermark {
    fn reserve(&self) -> u64 {
        let mut next = self.next.lock().expect("水位锁");
        let reserved = next.saturating_add(1);
        *next = reserved;
        reserved
    }

    /// 与 `compensate_watermark_after_log_failure` 同语义：水位退回落盘尾。
    fn compensate(&self, durable_tail: Option<u64>) {
        let mut next = self.next.lock().expect("水位锁");
        *next = durable_tail.unwrap_or(0);
    }
}

#[test]
fn concurrent_appends_never_reorder() {
    let dir = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(dir.path(), "session-session-concurrent");
    let watermark = Watermark {
        next: Mutex::new(0),
    };
    let conflicts = AtomicU32::new(0);
    let lock_timeouts = AtomicU32::new(0);
    let barrier = Barrier::new(WRITERS);

    std::thread::scope(|scope| {
        for writer in 0..WRITERS {
            let log = &log;
            let watermark = &watermark;
            let conflicts = &conflicts;
            let lock_timeouts = &lock_timeouts;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for index in 0..PER_WRITER {
                    let tag = format!("writer-{writer}-{index}");
                    // 本次要写的号：领到之后只有确认没写进去才重新领。
                    let mut reserved: Option<u64> = None;
                    let mut attempts = 0_u32;
                    loop {
                        attempts += 1;
                        assert!(
                            attempts < MAX_ATTEMPTS,
                            "补偿重试不得无限循环：tag={tag} attempts={attempts}"
                        );
                        let sequence = match reserved {
                            Some(sequence) => sequence,
                            None => {
                                let sequence = watermark.reserve();
                                reserved = Some(sequence);
                                sequence
                            }
                        };
                        let batch = [record(sequence, &tag)];
                        match axon_eventlog::append_once(log, sequence, 1_000, &batch) {
                            Ok(_) => break,
                            // 序号与文件尾不一致：退水位、按文件尾重算后重新领号再试。
                            Err(EventLogError::SequenceTailMismatch { .. }) => {
                                conflicts.fetch_add(1, Ordering::Relaxed);
                                watermark.compensate(tail_sequence(log));
                                reserved = None;
                            }
                            // 抢锁超时是瞬时竞争：一个字节都没写，号仍然有效，原样重试。
                            Err(EventLogError::Locked) => {
                                lock_timeouts.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(other) => panic!("并发追加只允许序号冲突或抢锁超时：{other:?}"),
                        }
                    }
                }
            });
        }
    });

    let snapshot = log.reader().expect("并发写入后整条日志必须可读");
    let records = snapshot.records();
    assert_eq!(records.len(), WRITERS * PER_WRITER, "补偿重试不得丢事件");
    let mut tags = BTreeSet::new();
    for (offset, stored) in records.iter().enumerate() {
        let expected = u64::try_from(offset).expect("条数") + 1;
        assert_eq!(stored.sequence, expected, "序号必须从 1 起严格递增且无洞");
        let tag = stored
            .payload
            .get("tag")
            .and_then(|value| value.as_str())
            .expect("载荷必须带 tag");
        assert!(tags.insert(tag.to_owned()), "同一事件不得重复落盘：{tag}");
    }
    let conflicts = conflicts.load(Ordering::Relaxed);
    assert!(
        conflicts > 0,
        "并发写入必须至少触发一次序号冲突，否则补偿路径没有被验证"
    );
    println!(
        "concurrent_appends writers={WRITERS} per_writer={PER_WRITER} conflicts={conflicts} lock_timeouts={}",
        lock_timeouts.load(Ordering::Relaxed)
    );
}
