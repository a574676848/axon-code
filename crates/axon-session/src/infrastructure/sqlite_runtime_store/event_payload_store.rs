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
        return std::fs::read_to_string(archive_root.join(reference));
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
    let mut file = File::open(archive_root.join(relative))?;
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

/// 该引用是否独占一个归档文件。
///
/// 分片被同一会话的多条事件共用，单条行过期不能删整片，回收交给文件保留期；
/// 历史单文件格式是一对一映射，行删掉即可连带删文件。
pub fn is_exclusive_payload_reference(reference: &str) -> bool {
    !reference.contains(REFERENCE_SPAN_SEPARATOR)
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
}
