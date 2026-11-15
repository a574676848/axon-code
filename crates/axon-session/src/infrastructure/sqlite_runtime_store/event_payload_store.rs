//! 事件正文的外部存储：分片 `.jsonl` 追加写与按偏移读取。
//!
//! 主库只承载定长元数据。流式帧这类高频小正文一旦内联，就会把 B-Tree 与 WAL
//! 一起撑大：实测单条 desktop 车道一天写入数十万帧，7 天保留期足以让主库涨到
//! 1.5 GiB 量级。因此流式事件一律外置，库里只留引用。
//!
//! 正文按会话分片追加写：同一会话每 `EVENT_PAYLOAD_SHARD_ROWS` 个序号一片，
//! 引用形如 `{session}/{shard}.jsonl@{offset}+{len}`。分片让文件数与「会话数 ×
//! 保留窗口内分片数」成正比，而不是与事件条数成正比：按 2000 条一片计，一条
//! 日写入五十万帧的车道一周也只有十几个分片文件。
//!
//! 引用同时兼容历史单文件格式 `{session}/{sequence}.json`：老库里的引用必须
//! 继续可读，不能因为换了存储形态就把历史帧读成缺失。

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use axon_domain::DaemonSessionId;

use super::SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES;

/// 每片容纳的事件序号跨度。
///
/// 正文平均 646 B 时一片约 1.3 MiB：既不让单个分片过大而拖慢尾部读取，
/// 也不把文件数推到百万级。
const EVENT_PAYLOAD_SHARD_ROWS: u64 = 2_000;
/// 流式帧的事件类型名。
const EVENT_TYPE_WIRE: &str = "wire_event";
/// 流式帧的内联上限（字节）：取 0 表示一律外置。
const WIRE_EVENT_INLINE_PAYLOAD_LIMIT_BYTES: usize = 0;
/// 引用中分隔「文件相对路径」与「偏移 + 长度」的标记。
const REFERENCE_SPAN_SEPARATOR: char = '@';
/// 引用中分隔偏移与长度的标记。
const REFERENCE_LENGTH_SEPARATOR: char = '+';

/// 把一条事件正文追加到会话分片，返回可用于回读的相对引用。
///
/// 返回 `None` 表示正文留在库里：写文件失败时宁可多占一点数据库空间，
/// 也不能让一条已经发生的运行事件落不了库。
pub(super) fn persist(
    archive_root: &Path,
    session_id: &DaemonSessionId,
    sequence: u64,
    event_type: &str,
    payload_json: &str,
) -> Option<String> {
    if payload_json.len() <= inline_limit_for(event_type) {
        return None;
    }
    let relative = format!(
        "{}/{:05}.jsonl",
        session_id.as_str(),
        sequence / EVENT_PAYLOAD_SHARD_ROWS
    );
    let path = archive_root.join(&relative);
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).ok()?;
    }
    // 追加前先取当前长度作为本帧起点：同一会话的事件写入在同一个 IMMEDIATE
    // 事务里串行推进，不存在并发追加同一条分片。
    let offset = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    file.write_all(payload_json.as_bytes()).ok()?;
    file.write_all(b"\n").ok()?;
    Some(format!(
        "{relative}{REFERENCE_SPAN_SEPARATOR}{offset}{REFERENCE_LENGTH_SEPARATOR}{}",
        payload_json.len()
    ))
}

/// 按引用取回事件正文。
///
/// 同时支持分片引用与历史单文件引用。调用方按 `std::io::ErrorKind::NotFound`
/// 区分「正文已被清理」与其他故障。
pub fn read(archive_root: &Path, reference: &str) -> std::io::Result<String> {
    let Some((relative, span)) = reference.split_once(REFERENCE_SPAN_SEPARATOR) else {
        return std::fs::read_to_string(payload_storage_path(archive_root, reference));
    };
    let invalid = |reason: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("外置正文引用{reason}：{reference}"),
        )
    };
    let (offset, length) = span
        .split_once(REFERENCE_LENGTH_SEPARATOR)
        .ok_or_else(|| invalid("缺少长度"))?;
    let offset: u64 = offset.parse().map_err(|_| invalid("偏移非法"))?;
    let length: usize = length.parse().map_err(|_| invalid("长度非法"))?;
    let mut file = File::open(payload_storage_path(archive_root, relative))?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0_u8; length];
    match file.read_exact(&mut buffer) {
        Ok(()) => {}
        // 偏移落在文件末尾之外说明这一片已被回收。normal 运行下 sequence 单调递增，
        // 旧分片区间不会被新事件重写；即便文件被重建，内容也不会落在旧偏移上。
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("外置正文已回收：{reference}"),
            ));
        }
        Err(error) => return Err(error),
    }
    String::from_utf8(buffer).map_err(|_| invalid("正文不是 UTF-8"))
}

/// 把 `{session}/{file}` 形式的相对引用逐段接到归档根下。
///
/// 不能直接用 `archive_root.join(reference)`：Windows 上归档根可能带 `\\?\`
/// verbatim 前缀，该前缀下 `/` 不是路径分隔符，整串会被当成一个文件名，
/// 于是文件明明存在也会读成 `InvalidInput` 一类的错误。读取与回收必须共用
/// 这一条解析规则，否则回收会静默删不掉文件、读侧却已经能读到。
pub fn payload_storage_path(archive_root: &Path, reference: &str) -> std::path::PathBuf {
    reference
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .fold(archive_root.to_path_buf(), |path, segment| {
            path.join(segment)
        })
}

/// 该引用是否独占一个归档文件。
///
/// 分片被同一会话的多条事件共用，单条行过期不能删整片，必须整片判定；历史单文件
/// 格式是一对一映射，行删掉即可连带删文件。
pub fn is_exclusive_payload_reference(reference: &str) -> bool {
    !reference.contains(REFERENCE_SPAN_SEPARATOR)
}

/// 回收整片都已被删除的外置正文分片，返回回收的文件数。
///
/// 分片按 `sequence / EVENT_PAYLOAD_SHARD_ROWS` 命名，一片覆盖 `[shard*行数, (shard+1)*行数)`
/// 这段序号区间。只有整段区间都早于该会话仍存活的最小序号时，片内才不可能有活着的
/// 行，此时整片回收安全；`min_live_sequence` 为 `None` 表示该会话已无任何存活行。
///
/// 只处理 `{shard}.jsonl`：`.jsonl.zst` 是归档产物、`{sequence}.json` 是历史单文件形态，
/// 都不归这条路径管。
pub fn reclaim_shards_before(
    archive_root: &Path,
    session_id: &str,
    min_live_sequence: Option<u64>,
) -> usize {
    let Ok(entries) = std::fs::read_dir(archive_root.join(session_id)) else {
        return 0;
    };
    let mut reclaimed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(shard) = name.strip_suffix(".jsonl") else {
            continue;
        };
        let Ok(shard_index) = shard.parse::<u64>() else {
            continue;
        };
        let shard_end = shard_index
            .saturating_add(1)
            .saturating_mul(EVENT_PAYLOAD_SHARD_ROWS);
        let dead = match min_live_sequence {
            Some(min_live) => shard_end <= min_live,
            None => true,
        };
        if dead && std::fs::remove_file(entry.path()).is_ok() {
            reclaimed += 1;
        }
    }
    reclaimed
}

fn inline_limit_for(event_type: &str) -> usize {
    if event_type == EVENT_TYPE_WIRE {
        WIRE_EVENT_INLINE_PAYLOAD_LIMIT_BYTES
    } else {
        SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::DaemonSessionId;

    fn session(id: &str) -> DaemonSessionId {
        DaemonSessionId::new(id).expect("会话标识")
    }

    /// 流式帧一律外置，其余事件仍按 4 KiB 阈值留在库里。
    #[test]
    fn wire_events_are_always_offloaded() {
        let directory = tempfile::tempdir().expect("临时目录");
        let root = directory.path();
        let lane = session("session-wire");

        let reference =
            persist(root, &lane, 1, "wire_event", "{\"text\":\"hi\"}").expect("流式帧必须外置");
        assert_eq!(read(root, &reference).expect("回读"), "{\"text\":\"hi\"}");

        assert!(
            persist(root, &lane, 2, "turn_completed", "{\"ok\":true}").is_none(),
            "非流式事件在小正文下仍应内联"
        );
    }

    /// 同一会话的多条事件共享分片，各自按偏移精确回读。
    #[test]
    fn frames_in_one_shard_are_read_back_by_offset() {
        let directory = tempfile::tempdir().expect("临时目录");
        let root = directory.path();
        let lane = session("session-shard");

        let first = persist(root, &lane, 1, "wire_event", "aaaa").expect("首帧");
        let second = persist(root, &lane, 2, "wire_event", "bbbbbb").expect("次帧");

        assert_eq!(
            std::fs::read_dir(root.join("session-shard"))
                .expect("分片目录")
                .count(),
            1,
            "同一分片区间只能产生一个文件"
        );
        assert_eq!(read(root, &first).expect("回读首帧"), "aaaa");
        assert_eq!(read(root, &second).expect("回读次帧"), "bbbbbb");
    }

    /// 序号跨越分片边界时落到下一个文件。
    #[test]
    fn frames_crossing_shard_boundary_use_next_file() {
        let directory = tempfile::tempdir().expect("临时目录");
        let root = directory.path();
        let lane = session("session-boundary");

        let head =
            persist(root, &lane, EVENT_PAYLOAD_SHARD_ROWS, "wire_event", "x").expect("边界前");
        let tail =
            persist(root, &lane, EVENT_PAYLOAD_SHARD_ROWS + 1, "wire_event", "y").expect("边界后");

        assert!(head.contains("00001.jsonl@"), "首片引用：{head}");
        assert!(tail.contains("00001.jsonl@"), "同一片内续写：{tail}");
        let next = persist(
            root,
            &lane,
            EVENT_PAYLOAD_SHARD_ROWS * 2 + 1,
            "wire_event",
            "z",
        )
        .expect("下一片");
        assert!(next.contains("00002.jsonl@"), "跨片后应换文件：{next}");
    }

    /// 历史单文件引用必须继续可读，否则换形态就毁掉存量回放。
    #[test]
    fn legacy_single_file_reference_stays_readable() {
        let directory = tempfile::tempdir().expect("临时目录");
        let root = directory.path();
        let directory_path = root.join("session-legacy");
        std::fs::create_dir_all(&directory_path).expect("旧目录");
        std::fs::write(directory_path.join("7.json"), "{\"legacy\":true}").expect("旧正文");

        assert_eq!(
            read(root, "session-legacy/7.json").expect("旧引用可读"),
            "{\"legacy\":true}"
        );
        assert!(is_exclusive_payload_reference("session-legacy/7.json"));
        assert!(!is_exclusive_payload_reference(
            "session-legacy/00000.jsonl@0+10"
        ));
    }

    /// 分片文件缺失时要报 NotFound，供调用方降级为占位记录。
    #[test]
    fn missing_shard_reports_not_found() {
        let directory = tempfile::tempdir().expect("临时目录");
        let error = read(directory.path(), "session-gone/00000.jsonl@0+4").expect_err("应当失败");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    /// 整片序号区间都早于保底线时回收该片，仍含存活行的分片必须保留。
    ///
    /// 缺了这条回收路径，行按保留期删掉了、正文却永久留在磁盘上：主库不再膨胀的
    /// 代价会被转移成无上限的归档目录增长。
    #[test]
    fn shards_before_the_live_floor_are_reclaimed() {
        let directory = tempfile::tempdir().expect("临时目录");
        let root = directory.path();
        let lane = session("session-reclaim");

        persist(root, &lane, 1, "wire_event", "aa").expect("0 号片");
        persist(
            root,
            &lane,
            EVENT_PAYLOAD_SHARD_ROWS * 2 + 1,
            "wire_event",
            "bb",
        )
        .expect("2 号片");

        // 保底线 4000：0 号片覆盖 [0,2000) 全部已删，2 号片覆盖 [4000,6000) 仍有活行。
        assert_eq!(reclaim_shards_before(root, lane.as_str(), Some(4000)), 1);
        let remaining = std::fs::read_dir(root.join("session-reclaim"))
            .expect("分片目录")
            .count();
        assert_eq!(remaining, 1, "只允许回收整片都已过期的那一个");

        // 会话已无任何存活行时，剩余分片一并回收。
        assert_eq!(reclaim_shards_before(root, lane.as_str(), None), 1);
        assert!(
            std::fs::read_dir(root.join("session-reclaim"))
                .expect("分片目录")
                .next()
                .is_none(),
            "无存活行时必须清空分片"
        );
    }
}
