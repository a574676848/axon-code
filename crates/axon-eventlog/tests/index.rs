//! 帧索引的契约测试：索引是派生数据，只在「能省下解压」时被采用。
//!
//! 三条不可回退的性质：
//! 1. 有索引与没有索引读到的结果逐条相同；
//! 2. 索引缺失、落后或损坏时读路径退回单趟顺序解压，绝不报错或返回错页；
//! 3. 写句柄每次追加都让索引覆盖到日志真实末字节，崩溃修复后也不会留空洞。

use std::path::{Path, PathBuf};

use axon_eventlog::{
    EventLog, RuntimeEventKind, RuntimeEventRecord, read_after, read_interaction_after,
    read_interaction_tail, read_tail_matching, read_tail_record,
};
use serde_json::json;

fn record(sequence: u64) -> RuntimeEventRecord {
    RuntimeEventRecord::new(
        sequence,
        1_700_000_000_000 + sequence,
        RuntimeEventKind::ToolCall,
        json!({ "sequence": sequence }),
    )
}

/// 建一条包含 `frames` 帧、每帧 5 条事件的日志。
fn seed_log(directory: &Path, frames: u64) -> (EventLog, PathBuf) {
    let log = EventLog::open(directory, "session-session-index");
    let mut writer = log.writer().expect("写句柄");
    let mut sequence = 0_u64;
    for _ in 0..frames {
        let batch: Vec<RuntimeEventRecord> = (0..5)
            .map(|_| {
                let record = record(sequence);
                sequence = sequence.saturating_add(1);
                record
            })
            .collect();
        writer.append_batch(&batch).expect("批次写入");
    }
    let path = log.path();
    (log, path)
}

fn index_path(log_path: &Path) -> PathBuf {
    let mut name = log_path.as_os_str().to_os_string();
    name.push(".idx");
    PathBuf::from(name)
}

/// 全量快照里的序号序列，作为"正确答案"。
fn snapshot_sequences(path: &Path) -> Vec<u64> {
    axon_eventlog::EventLogReader::open(path)
        .expect("全量读者")
        .records()
        .iter()
        .map(|record| record.sequence)
        .collect()
}

#[test]
fn append_writes_an_index_that_covers_the_log() {
    let directory = tempfile::tempdir().expect("临时目录");
    let (_log, path) = seed_log(directory.path(), 6);
    let index = index_path(&path);
    assert!(index.is_file(), "追加后必须留下帧索引");

    let log_len = std::fs::metadata(&path).expect("日志元数据").len();
    assert!(log_len > 0);
    // 覆盖长度是唯一的新鲜度判据：读路径靠它判断索引是否可信。
    let bytes = std::fs::read(&index).expect("读取索引");
    let covered = u64::from_le_bytes(bytes[16..24].try_into().expect("覆盖长度字段"));
    assert_eq!(covered, log_len, "索引必须覆盖到日志真实末字节");
}

#[test]
fn index_and_fallback_reads_return_the_same_pages() {
    let directory = tempfile::tempdir().expect("临时目录");
    let (_log, path) = seed_log(directory.path(), 8);
    let expected = snapshot_sequences(&path);

    for (after, limit) in [(0_u64, 7_usize), (3, 100), (17, 3), (39, 10), (40, 10)] {
        let expected_page: Vec<u64> = expected
            .iter()
            .copied()
            .filter(|sequence| *sequence > after)
            .take(limit)
            .collect();
        let with_index = read_after(&path, after, limit).expect("有索引读取");
        let with_index: Vec<u64> = with_index.iter().map(|record| record.sequence).collect();
        assert_eq!(
            with_index, expected_page,
            "有索引的分页结果必须与全量一致：after={after} limit={limit}"
        );

        // 删掉索引重跑同一组读取：结果必须逐条相同，只是走单趟顺序解压。
        if index_path(&path).is_file() {
            std::fs::remove_file(index_path(&path)).expect("删除索引");
        }
        let without = read_after(&path, after, limit).expect("无索引读取");
        let without: Vec<u64> = without.iter().map(|record| record.sequence).collect();
        assert_eq!(
            without, expected_page,
            "索引缺失不得改变分页结果：after={after} limit={limit}"
        );
    }
}

#[test]
fn stale_index_is_ignored_and_rebuilt_by_the_next_append() {
    let directory = tempfile::tempdir().expect("临时目录");
    let (log, path) = seed_log(directory.path(), 4);
    let expected = snapshot_sequences(&path);
    // 覆盖长度被改小 = 索引落后于日志：读路径必须忽略它而不是给出旧页。
    let index = index_path(&path);
    let mut bytes = std::fs::read(&index).expect("读取索引");
    bytes[16..24].copy_from_slice(&1_u64.to_le_bytes());
    std::fs::write(&index, &bytes).expect("写入落伍索引");

    let read = read_after(&path, 0, 5).expect("落后索引下读取");
    let read: Vec<u64> = read.iter().map(|record| record.sequence).collect();
    // after 是排他游标：序号 0 不在第一页里。
    assert_eq!(
        read,
        expected
            .iter()
            .copied()
            .filter(|sequence| *sequence > 0)
            .take(5)
            .collect::<Vec<_>>()
    );

    // 下一次追加由写句柄重建索引：覆盖长度回到日志真实长度。
    let mut writer = log.writer().expect("重新打开写句柄");
    let next = expected.last().copied().unwrap_or(0).saturating_add(1);
    writer.append_batch(&[record(next)]).expect("继续追加一帧");
    drop(writer);
    let bytes = std::fs::read(&index).expect("读取重建后的索引");
    let covered = u64::from_le_bytes(bytes[16..24].try_into().expect("覆盖长度字段"));
    assert_eq!(
        covered,
        std::fs::metadata(&path).expect("日志元数据").len(),
        "追加后索引必须重新覆盖到日志末字节"
    );
}

#[test]
fn torn_tail_repair_keeps_the_index_consistent() {
    let directory = tempfile::tempdir().expect("临时目录");
    let (log, path) = seed_log(directory.path(), 3);
    let expected = snapshot_sequences(&path);

    // 模拟崩溃留下的撕裂尾：把末帧截掉一段，留一个不完整的真实帧。
    {
        let metadata = std::fs::metadata(&path).expect("日志元数据");
        let truncated = metadata.len().saturating_sub(12);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("打开以截断");
        file.set_len(truncated).expect("截断末帧");
        file.sync_data().expect("落盘");
    }

    // 重新打开写句柄：先截断撕裂尾，索引随覆盖长度回到截断后的真实长度。
    let mut writer = log.writer().expect("修复撕裂尾后应可写");
    let repaired = snapshot_sequences(&path);
    assert!(
        repaired.len() < expected.len(),
        "撕裂尾必须被截掉，而不是当成完整帧保留"
    );
    let next = writer.next_sequence();
    assert_eq!(
        next,
        repaired.last().copied().unwrap_or(0).saturating_add(1),
        "续写序号必须接在修复后的末条之后，不能沿用截断前的序号"
    );
    writer
        .append_batch(&[record(next)])
        .expect("修复后继续追加");
    drop(writer);

    let sequences = snapshot_sequences(&path);
    assert_eq!(
        sequences.len(),
        repaired.len().saturating_add(1),
        "只有修复后新写的一帧应当增加"
    );
    let read = read_after(&path, next.saturating_sub(1), 10).expect("读取尾部");
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].sequence, next);
}

#[test]
fn append_still_succeeds_when_the_index_path_is_blocked() {
    let directory = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(directory.path(), "session-session-index-blocked");
    // 把索引路径占成目录：写句柄打不开它，但事件仍必须落盘。
    std::fs::create_dir_all(index_path(&log.path())).expect("占用索引路径");
    let mut writer = log.writer().expect("索引不可用不应挡住写句柄");
    writer
        .append_batch(&[record(0), record(1)])
        .expect("事件必须照常落盘");
    drop(writer);

    let sequences = snapshot_sequences(&log.path());
    assert_eq!(sequences, vec![0, 1], "索引降级不得影响事件事实");
    let read = read_after(&log.path(), 0, 10).expect("无索引读取");
    assert_eq!(read.len(), 1, "after 是排他游标：只返回序号 1");
}

#[test]
fn interaction_reads_match_full_scan_filtering() {
    let directory = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(directory.path(), "session-session-interaction");
    let path = log.path();
    // 两条交互交替写入：索引里的交互指纹必须能把两者分开。
    let mut writer = log.writer().expect("写句柄");
    let mut sequence = 0_u64;
    for round in 0..6 {
        let interaction = if round % 2 == 0 {
            "interaction-a"
        } else {
            "interaction-b"
        };
        let batch: Vec<RuntimeEventRecord> = (0..4)
            .map(|_| {
                let record = record(sequence).with_interaction(interaction);
                sequence = sequence.saturating_add(1);
                record
            })
            .collect();
        writer.append_batch(&batch).expect("批次写入");
    }
    drop(writer);

    let all = snapshot_sequences(&path);
    for interaction in ["interaction-a", "interaction-b"] {
        for (after, limit) in [(0_u64, 5_usize), (7, 5), (20, 100), (23, 4)] {
            let expected: Vec<u64> = all
                .iter()
                .copied()
                .filter(|sequence| *sequence > after)
                .collect::<Vec<_>>();
            // 期望值按同一份归属规则过滤后再截断。
            let expected = expected
                .into_iter()
                .filter(|sequence| {
                    let even_round = (*sequence / 4) % 2 == 0;
                    (interaction == "interaction-a") == even_round
                })
                .take(limit)
                .collect::<Vec<_>>();
            let actual: Vec<u64> = read_interaction_after(&path, interaction, after, limit)
                .expect("按键读取")
                .iter()
                .map(|record| record.sequence)
                .collect();
            assert_eq!(
                actual, expected,
                "按键分页必须与整会话过滤一致：{interaction} after={after} limit={limit}"
            );
        }
    }

    // 删掉索引后同一组断言必须仍然成立（退化路径）。
    std::fs::remove_file(index_path(&path)).expect("删除索引");
    let actual: Vec<u64> = read_interaction_after(&path, "interaction-a", 0, 100)
        .expect("无索引按键读取")
        .iter()
        .map(|record| record.sequence)
        .collect();
    assert_eq!(actual, vec![1, 2, 3, 8, 9, 10, 11, 16, 17, 18, 19]);
    // 删索引后必须由写句柄重建，读取路径不写文件。
    assert!(!index_path(&path).is_file(), "读路径不得擅自重建索引");
}

#[test]
fn mixed_batch_is_indexed_as_cross_interaction() {
    let directory = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(directory.path(), "session-session-mixed");
    let mut writer = log.writer().expect("写句柄");
    writer
        .append_batch(&[
            record(0).with_interaction("interaction-a"),
            record(1).with_interaction("interaction-b"),
        ])
        .expect("跨交互批次");
    writer
        .append_batch(&[record(2).with_interaction("interaction-a")])
        .expect("单交互批次");
    drop(writer);

    // 跨交互帧按 0 记账，读侧必须保守扫过它而不是漏掉其中的记录。
    // after 是排他游标：跨交互帧里的序号 0 不在 after=0 的页里。
    let a: Vec<u64> = read_interaction_after(&log.path(), "interaction-a", 0, 10)
        .expect("按键读取")
        .iter()
        .map(|record| record.sequence)
        .collect();
    assert_eq!(a, vec![2], "跨交互帧里的目标记录不得漏读");
    let b: Vec<u64> = read_interaction_after(&log.path(), "interaction-b", 0, 10)
        .expect("按键读取")
        .iter()
        .map(|record| record.sequence)
        .collect();
    assert_eq!(b, vec![1]);
}

#[test]
fn interaction_tail_returns_the_last_window() {
    let directory = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(directory.path(), "session-session-tail-window");
    let mut writer = log.writer().expect("写句柄");
    let mut sequence = 0_u64;
    for round in 0..5 {
        let interaction = if round % 2 == 0 {
            "interaction-a"
        } else {
            "interaction-b"
        };
        let batch: Vec<RuntimeEventRecord> = (0..2)
            .map(|_| {
                let record = record(sequence).with_interaction(interaction);
                sequence = sequence.saturating_add(1);
                record
            })
            .collect();
        writer.append_batch(&batch).expect("批次写入");
    }
    drop(writer);

    // 每轮一个批次两条事件：a 拿到 0,1 / 4,5 / 8,9，尾窗三条即 5,8,9。
    let tail = read_interaction_tail(&log.path(), "interaction-a", 3).expect("按键尾窗");
    let sequences: Vec<u64> = tail.iter().map(|record| record.sequence).collect();
    assert_eq!(sequences, vec![5, 8, 9], "尾窗按序号升序且只含目标交互");
    assert!(
        tail.iter()
            .all(|record| { record.interaction_id.as_deref() == Some("interaction-a") }),
        "尾窗不得混入别条交互"
    );
}

#[test]
fn legacy_payload_ownership_is_honoured_without_a_column() {
    let directory = tempfile::tempdir().expect("临时目录");
    let log = EventLog::open(directory.path(), "session-session-legacy");
    let path = log.path();
    // 老记录只把归属写在载荷里：归属列始终为空，模拟升级前写入的事件。
    let legacy = |sequence: u64, interaction: &str| {
        RuntimeEventRecord::new(
            sequence,
            1_700_000_000_000 + sequence,
            RuntimeEventKind::ToolCall,
            json!({ "interactionId": interaction }),
        )
    };
    {
        let mut writer = log.writer().expect("写句柄");
        writer
            .append_batch(&[legacy(0, "interaction-old"), legacy(1, "interaction-other")])
            .expect("老记录批次");
        writer
            .append_batch(&[legacy(2, "interaction-old")])
            .expect("老记录批次");
    }

    // 按键读取必须能从载荷认出归属；删掉索引走退化路径也要一致。
    let keyed: Vec<u64> = read_interaction_after(&path, "interaction-old", 0, 10)
        .expect("按键读取")
        .iter()
        .map(|record| record.sequence)
        .collect();
    assert_eq!(keyed, vec![2], "after 排他，且只认这条交互的记录");

    std::fs::remove_file(index_path(&path)).expect("删除索引");
    let fallback: Vec<u64> = read_interaction_after(&path, "interaction-old", 0, 10)
        .expect("退化路径")
        .iter()
        .map(|record| record.sequence)
        .collect();
    assert_eq!(fallback, keyed, "两条路径的归属判定必须同口径");
}

#[test]
fn tail_reads_work_with_and_without_the_index() {
    let directory = tempfile::tempdir().expect("临时目录");
    let (_log, path) = seed_log(directory.path(), 6);
    let expected = snapshot_sequences(&path);
    let tail = expected.last().copied();

    assert_eq!(
        read_tail_record(&path)
            .expect("有索引读末条")
            .map(|record| record.sequence),
        tail,
        "末条序号必须取自末帧"
    );
    std::fs::remove_file(index_path(&path)).expect("删除索引");
    assert_eq!(
        read_tail_record(&path)
            .expect("无索引读末条")
            .map(|record| record.sequence),
        tail,
        "索引缺失时末条序号口径不变"
    );

    // 尾窗按谓词反向取：两种路径结果一致。
    let predicate = |record: &RuntimeEventRecord| record.sequence.is_multiple_of(3);
    let expected_matched: Vec<u64> = expected
        .iter()
        .copied()
        .filter(|sequence| sequence.is_multiple_of(3))
        .collect();
    let tail_window = read_tail_matching(&path, 4, predicate).expect("无索引尾窗");
    let tail_window: Vec<u64> = tail_window.iter().map(|record| record.sequence).collect();
    assert_eq!(
        tail_window,
        expected_matched[expected_matched.len().saturating_sub(4)..].to_vec()
    );
}
