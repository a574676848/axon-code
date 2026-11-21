//! Thread 事件正文外置：超阈值的事件正文搬到独立正文文件，日志行只留引用。
//!
//! 为什么要这层：Thread 事件日志与分片存储共享「单条记录有界」的存储范式（shard 目标
//! 8 MiB、单条硬上限 64 MiB），但换取窗口时写入的 checkpoint 记录承载整份
//! `ThreadDetail`，体量是 O(整个会话)，长会话必然越过硬上限，于是换窗在「投影落库」
//! 这一步被存储层拒绝并整体回滚。把正文外置之后，单条日志记录恒为小对象，checkpoint
//! 不再受全量历史支配。
//!
//! 与会话事件正文归档同构：正文用 `axon_eventlog::encode_frame`（zstd、带校验和）
//! 落成独立文件，引用里带摘要与明文字节数；读侧按摘要校验后透明还原。差异只有两点：
//!
//! - 引用写在 `event` 字段里（`type` 为 [`THREAD_EVENT_BODY_ARCHIVED_TYPE`]），
//!   记录头（schema/thread/sequence/recorded）保持原样，序号与索引投影不受影响；
//! - 正文缺失或摘要不符一律报错，不做降级：Thread 历史不可丢，静默跳过只会把
//!   「读不出来」伪装成「这一轮本来就没有内容」。

use std::{
    fs,
    io::Write as _,
    path::{Component, Path, PathBuf},
};

use axon_eventlog::{decode_frame, encode_frame, scan_frames};

use crate::domain::ThreadEvent;

use super::jsonl_store::JsonlThreadEventStore;

/// 单条 Thread 事件的内联体量上限。
///
/// 取 1 MiB：分片预算 8 MiB 的八分之一，单条硬上限 64 MiB 的六十四分之一。超过就说明
/// 这条记录承载的是「正文」而不是「状态」，必须外置，否则日志体量会随会话线性膨胀。
pub(crate) const THREAD_EVENT_INLINE_BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// 外置正文的引用类型名：日志行里以它替代真实事件。
pub(crate) const THREAD_EVENT_BODY_ARCHIVED_TYPE: &str = "thread_event_body_archived";

/// 正文目录名，落在 Thread 事件存储的同级。
const BODY_DIR_NAME: &str = "thread-bodies";
/// 正文文件后缀：单条事件一个 zstd 帧。
const BODY_SUFFIX: &str = ".json.zst";
/// 正文写入的临时文件后缀。
const BODY_TMP_SUFFIX: &str = ".tmp";
/// 单个 zstd 帧的明文分块上限。
///
/// 事件帧解码侧有 64 MiB 明文硬上限（`axon_eventlog` 的既有边界），而一条 checkpoint
/// 正文可能远超它；因此正文按块压成多个帧并存进同一个文件，读侧逐帧还原后拼接。
/// 取 4 MiB：远低于上限，也让单帧解压的内存峰值保持常量。
const BODY_CHUNK_PLAIN_BYTES: usize = 4 * 1024 * 1024;

/// 一条外置正文的引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadEventBodyRef {
    /// 明文字节数，仅用于诊断与校验提示。
    pub(crate) bytes: u64,
    /// zstd 帧的 SHA-256；读侧据此判定正文是否被替换或截断。
    pub(crate) digest: String,
    /// 相对正文根的路径，不含机器相关的绝对路径。
    pub(crate) path: String,
}

impl ThreadEventBodyRef {
    /// 转成日志行里的 `event` 对象。
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "type": THREAD_EVENT_BODY_ARCHIVED_TYPE,
            "bytes": self.bytes,
            "digest": self.digest,
            "path": self.path,
        })
    }

    /// 从日志行的 `event` 对象还原引用；不是引用时返回 `None`。
    pub(crate) fn from_json(value: &serde_json::Value) -> Option<Self> {
        let kind = value.get("type").and_then(serde_json::Value::as_str)?;
        if kind != THREAD_EVENT_BODY_ARCHIVED_TYPE {
            return None;
        }
        Some(Self {
            bytes: value.get("bytes").and_then(serde_json::Value::as_u64)?,
            digest: value
                .get("digest")
                .and_then(serde_json::Value::as_str)?
                .to_owned(),
            path: value
                .get("path")
                .and_then(serde_json::Value::as_str)?
                .to_owned(),
        })
    }
}

/// 正文根目录：`<data>/sessions/thread-bodies`。
pub(crate) fn body_root(root: &Path) -> PathBuf {
    JsonlThreadEventStore::path(root)
        .parent()
        .map(|parent| parent.join(BODY_DIR_NAME))
        .unwrap_or_else(|| root.join(BODY_DIR_NAME))
}

/// 某个 thread 的正文目录。
fn thread_body_dir(root: &Path, thread_id: &str) -> PathBuf {
    body_root(root).join(JsonlThreadEventStore::thread_prefix(thread_id))
}

/// 正文文件名：按事件序号一一对应。
fn body_file_name(sequence: u64) -> String {
    format!("{sequence:020}{BODY_SUFFIX}")
}

/// 一条记录是否需要外置正文。
pub(crate) fn exceeds_inline_limit(encoded_len: usize) -> bool {
    encoded_len > THREAD_EVENT_INLINE_BODY_LIMIT_BYTES
}

/// 把事件正文写进正文文件，返回可写进日志行的引用。
pub(crate) fn externalize(
    root: &Path,
    thread_id: &str,
    sequence: u64,
    event: &ThreadEvent,
) -> Result<ThreadEventBodyRef, String> {
    let plain = serde_json::to_vec(event).map_err(|_| "thread 事件正文序列化失败".to_owned())?;
    let mut frame = Vec::new();
    for chunk in plain.chunks(BODY_CHUNK_PLAIN_BYTES) {
        frame.extend(
            encode_frame(chunk).map_err(|error| format!("thread 事件正文压缩失败：{error}"))?,
        );
    }
    let digest = axon_crypto::sha256_hex(&frame);
    let directory = thread_body_dir(root, thread_id);
    fs::create_dir_all(&directory).map_err(|_| "无法创建 thread 正文目录".to_owned())?;
    let name = body_file_name(sequence);
    let target = directory.join(&name);
    let temporary = directory.join(format!("{name}{BODY_TMP_SUFFIX}"));
    {
        let mut file =
            fs::File::create(&temporary).map_err(|_| "无法创建 thread 正文临时文件".to_owned())?;
        file.write_all(&frame)
            .map_err(|_| "无法写入 thread 正文临时文件".to_owned())?;
        file.sync_all()
            .map_err(|_| "无法同步 thread 正文临时文件".to_owned())?;
    }
    fs::rename(&temporary, &target).map_err(|_| "无法替换 thread 正文文件".to_owned())?;
    Ok(ThreadEventBodyRef {
        bytes: u64::try_from(plain.len()).unwrap_or(u64::MAX),
        digest,
        path: format!("{}/{name}", JsonlThreadEventStore::thread_prefix(thread_id)),
    })
}

/// 按引用还原事件正文。
///
/// 引用路径只接受「本 thread 的正文目录 + 序号文件名」这一种形态：日志文件可能被外部
/// 改动，路径穿越必须在这里被挡住，而不是靠调用方自觉。
pub(crate) fn resolve(
    root: &Path,
    thread_id: &str,
    sequence: u64,
    reference: &ThreadEventBodyRef,
) -> Result<ThreadEvent, String> {
    let expected = format!(
        "{}/{}",
        JsonlThreadEventStore::thread_prefix(thread_id),
        body_file_name(sequence)
    );
    if reference.path != expected || !reference.path_is_safe() {
        return Err(format!(
            "thread 正文引用不合法：期望 {expected}，实际 {}",
            reference.path
        ));
    }
    let path = thread_body_dir(root, thread_id).join(body_file_name(sequence));
    let frame = fs::read(&path).map_err(|error| {
        format!(
            "thread 正文缺失（{}）：{error}",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        )
    })?;
    let digest = axon_crypto::sha256_hex(&frame);
    if digest != reference.digest {
        return Err(format!(
            "thread 正文校验失败：摘要不符（期望 {}，实际 {digest}）",
            reference.digest
        ));
    }
    let mut plain = Vec::new();
    let scan = scan_frames(&frame).map_err(|error| format!("thread 正文解压失败：{error}"))?;
    if scan.torn_start.is_some() {
        return Err("thread 正文末尾不完整".to_owned());
    }
    for span in &scan.frames {
        let chunk = decode_frame(&frame[span.start..span.end])
            .map_err(|error| format!("thread 正文解压失败：{error}"))?;
        plain.extend(chunk);
    }
    if u64::try_from(plain.len()).unwrap_or(u64::MAX) != reference.bytes {
        return Err(format!(
            "thread 正文校验失败：字节数不符（期望 {}，实际 {}）",
            reference.bytes,
            plain.len()
        ));
    }
    serde_json::from_slice::<ThreadEvent>(&plain)
        .map_err(|error| format!("thread 正文解析失败：{error}"))
}

impl ThreadEventBodyRef {
    /// 引用路径只允许相对路径，不允许出现父级跳转。
    fn path_is_safe(&self) -> bool {
        let path = Path::new(&self.path);
        !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
    }
}

impl JsonlThreadEventStore {
    /// thread 正文目录的字节数：治理与容量统计用它把「外置正文」算进会话体量。
    pub fn thread_body_bytes(root: &Path, thread_id: &str) -> Result<u64, String> {
        let directory = thread_body_dir(root, thread_id);
        if !directory.exists() {
            return Ok(0);
        }
        let entries =
            fs::read_dir(&directory).map_err(|_| "无法读取 thread 正文目录".to_owned())?;
        let mut total = 0_u64;
        for entry in entries {
            let entry = entry.map_err(|_| "无法读取 thread 正文条目".to_owned())?;
            if entry.path().is_file() {
                total = total.saturating_add(entry.metadata().map(|meta| meta.len()).unwrap_or(0));
            }
        }
        Ok(total)
    }
}

/// 删除某个 thread 的全部正文；目录不存在按已清理处理。
pub(crate) fn remove_bodies(root: &Path, thread_id: &str) -> Result<(), String> {
    let directory = thread_body_dir(root, thread_id);
    if !directory.exists() {
        return Ok(());
    }
    fs::remove_dir_all(&directory).map_err(|error| format!("删除 thread 正文目录失败：{error}"))
}

/// 只保留指定序号的正文，其余删除。
///
/// checkpoint 重写会把整份历史替换成一条记录，旧事件的正文随之失去引用；不清掉它们，
/// 磁盘上就会留下永远读不到的正文副本。
pub(crate) fn prune_bodies_except(
    root: &Path,
    thread_id: &str,
    keep_sequence: u64,
) -> Result<(), String> {
    prune_bodies_to(root, thread_id, &[keep_sequence])
}

/// 只保留给定序号的正文；管理型重写用它把仍然有效的正文留下。
pub(crate) fn prune_bodies_to(
    root: &Path,
    thread_id: &str,
    keep_sequences: &[u64],
) -> Result<(), String> {
    let directory = thread_body_dir(root, thread_id);
    if !directory.exists() {
        return Ok(());
    }
    let keep = keep_sequences
        .iter()
        .map(|sequence| body_file_name(*sequence))
        .collect::<Vec<_>>();
    let entries = fs::read_dir(&directory).map_err(|_| "无法读取 thread 正文目录".to_owned())?;
    for entry in entries {
        let entry = entry.map_err(|_| "无法读取 thread 正文条目".to_owned())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if keep.iter().any(|candidate| candidate == &name) {
            continue;
        }
        if entry.path().is_file() {
            fs::remove_file(entry.path())
                .map_err(|error| format!("删除 thread 正文文件失败：{error}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "thread_event_body_tests.rs"]
mod tests;
