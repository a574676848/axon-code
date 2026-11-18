//! 会话事件日志：zstd 分帧的 append-only 存储。
//!
//! 物理形态是一个文件，内部由若干**互相独立的 zstd 帧**串接而成：
//!
//! - 第 0 帧只装一行 header，记录格式版本与会话标识；
//! - 此后每个追加批次一帧，帧内是若干行 JSONL 事件；
//! - 每帧独立压缩并带内容校验和，因此崩溃只会损坏最后一帧。
//!
//! 设计目标见 `docs/dev/storage-architecture.md`：事件流不占用 SQLite 写锁，
//! 单次追加只做一次顺序写与一次 `fsync`。

#![deny(missing_docs)]

mod append;
mod error;
mod format;
mod index;
mod lock;
mod read;
mod record;
mod tail;

pub use append::{EventLogWriter, append_once};
pub use error::EventLogError;
pub use format::{FrameScan, FrameSpan, HEADER_FRAME_INDEX, Header, encode_frame};
pub use read::{
    EventLogReader, EventLogSnapshot, read_after, read_interaction_after, read_interaction_tail,
    read_tail_matching, read_tail_record, read_tail_records, read_tail_scan,
};
pub use record::{
    JobOutcome, JobRef, RuntimeEventKind, RuntimeEventRecord, TurnMetrics, TurnOrigin, TurnOutcome,
};
pub use tail::{InteractionRecord, read_latest_per_interaction};

use std::path::{Path, PathBuf};

/// 官方会话事件目录名。
pub const EVENTS_DIR: &str = "events";
/// 当前物理格式代数。
pub const FORMAT_VERSION: u32 = 1;
/// 单个逻辑会话的日志文件名前缀。
pub const LOG_FILE_STEM: &str = "session";

/// 一个会话的事件日志句柄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLog {
    root: PathBuf,
    session_id: String,
}

impl EventLog {
    /// 在给定根目录下打开（不创建）某个会话的日志。
    #[must_use]
    pub fn open(root: impl Into<PathBuf>, session_id: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            session_id: session_id.into(),
        }
    }

    /// 事件根目录。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 会话标识。
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 当前代数的物理路径。
    ///
    /// 目录名使用十六进制转义，规避 Windows 路径长度与保留字符问题。
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.root
            .join(Self::encode_segment(&self.session_id))
            .join(format!("{LOG_FILE_STEM}.v{FORMAT_VERSION}.jsonl.zstd"))
    }

    /// 追加写句柄。同一时刻只允许一个写句柄持有该日志。
    pub fn writer(&self) -> Result<EventLogWriter, EventLogError> {
        EventLogWriter::open(self)
    }

    /// 只读快照。
    pub fn reader(&self) -> Result<EventLogReader, EventLogError> {
        EventLogReader::open(&self.path())
    }

    /// 会话目录名的稳定编码。
    ///
    /// 只保留 ASCII 字母数字与 `-`、`_`，其余按 `_XX` 十六进制转义，
    /// 因此对任意 UTF-8 会话标识都是单射，不会出现路径穿越。
    #[must_use]
    pub fn encode_segment(raw: &str) -> String {
        let mut encoded = String::with_capacity(raw.len());
        for byte in raw.as_bytes() {
            let safe = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_');
            if safe {
                encoded.push(char::from(*byte));
            } else {
                encoded.push('_');
                let _ = std::fmt::Write::write_fmt(&mut encoded, format_args!("{byte:02X}"));
            }
        }
        if encoded.is_empty() {
            "_".to_owned()
        } else {
            encoded
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_encoding_is_injective_and_path_safe() {
        let cases = ["session-session-abc", "a/b", "a\\b", "..", "会话一", ""];
        let mut seen = std::collections::BTreeSet::new();
        for case in cases {
            let encoded = EventLog::encode_segment(case);
            assert!(!encoded.contains('/'), "不得包含路径分隔符：{encoded}");
            assert!(!encoded.contains('\\'), "不得包含反斜杠：{encoded}");
            assert!(seen.insert(encoded), "编码必须单射");
        }
    }

    #[test]
    fn path_uses_current_generation() {
        let log = EventLog::open("/tmp/events", "session-session-1");
        let text = log.path().to_string_lossy().replace('\\', "/");
        assert!(
            text.ends_with("/session-session-1/session.v1.jsonl.zstd"),
            "{text}"
        );
    }

    #[test]
    fn ulid_session_id_is_its_own_directory_name() {
        // 事件分段目录名必须等于会话主键：ULID 只含 ASCII 大写字母与数字，
        // 编码器不产生任何转义，因此目录名与主键逐字相同。
        let session_id = "01J8ZQ7K3M4N5P6Q7R8S9T0V1W";
        let log = EventLog::open("/tmp/events", session_id);
        let text = log.path().to_string_lossy().replace('\\', "/");
        assert_eq!(EventLog::encode_segment(session_id), session_id);
        assert!(
            text.ends_with(&format!("/{session_id}/session.v1.jsonl.zstd")),
            "事件目录必须就是主键：{text}"
        );
    }

    #[test]
    fn distinct_session_ids_never_share_a_segment() {
        // 目录名是查找键：不同主键落到同一目录就等于串了日志。
        let ids = [
            "01J8ZQ7K3M4N5P6Q7R8S9T0V1W",
            "01J8ZQ7K3M4N5P6Q7R8S9T0V1X",
            "session-1",
            "session_1",
            "session_1_2F",
        ];
        let mut seen = std::collections::BTreeMap::new();
        for id in ids {
            let segment = EventLog::encode_segment(id);
            if let Some(previous) = seen.insert(segment.clone(), id) {
                panic!("会话 {previous} 与 {id} 共用目录 {segment}");
            }
        }
    }
}
