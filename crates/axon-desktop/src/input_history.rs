//! 输入历史：与 TUI 共用同一份 composer draft 历史。
//!
//! 事实源只有一个：`~/.axon/data/runtime/draft_history.jsonl`。TUI 的
//! `history_store` 写的就是这一份，桌面端只读写同一个文件，因此「在终端里发过的
//! 消息，桌面上按上键也能取回」，也不存在第二套历史需要同步。
//!
//! 归一化规则与 TUI 逐条一致：丢掉空串、按内容去重（保留最后一次出现的位置）、
//! 上限 1000 条。物理分片由 `axon_jsonl` 负责，这里只认逻辑路径。

use std::path::PathBuf;

use axon_config::{ConfigLoader, USER_RUNTIME_DATA_DIR, USER_RUNTIME_SUBDIR};
use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};

/// 逻辑文件名；与 TUI 的 `DRAFT_HISTORY_FILE` 一致。
const DRAFT_HISTORY_FILE: &str = "draft_history.jsonl";
/// 保留条数上限；与 TUI 的 `DRAFT_HISTORY_LIMIT` 一致。
const DRAFT_HISTORY_LIMIT: usize = 1_000;
/// Composer 历史使用的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

/// 一条历史记录；字段名必须与 TUI 写出的 JSONL 一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryRecord {
    /// 用户当时输入的正文。
    text: String,
}

/// 历史文件路径：`<axon 根>/data/runtime/draft_history.jsonl`。
fn history_path() -> Result<PathBuf, String> {
    let root = ConfigLoader::default_user_axon_root_path()
        .map_err(|error| format!("无法解析 Axon 运行态根目录：{error:?}"))?;
    Ok(root
        .join(USER_RUNTIME_DATA_DIR)
        .join(USER_RUNTIME_SUBDIR)
        .join(DRAFT_HISTORY_FILE))
}

/// 读取历史，并按 TUI 的规则归一化。
pub fn list_input_history() -> Result<Vec<String>, String> {
    let path = history_path()?;
    let lines = SHARD_STORE
        .read_lines(&path)
        .map_err(|error| format!("读取输入历史失败：{error}"))?;
    let mut entries: Vec<String> = lines
        .iter()
        .filter_map(|line| serde_json::from_slice::<HistoryRecord>(line).ok())
        .map(|record| record.text)
        .collect();
    normalize(&mut entries);
    Ok(entries)
}

/// 追加一条历史并落盘，返回归一化之后的完整历史。
///
/// 与 TUI 的 `record_input_history` 一致：与最后一条相同时直接跳过，
/// 不做无意义的整文件重写。
pub fn append_input_history(text: &str) -> Result<Vec<String>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return list_input_history();
    }
    let path = history_path()?;
    let mut entries = list_input_history()?;
    if entries.last().is_some_and(|entry| entry == trimmed) {
        return Ok(entries);
    }
    entries.push(trimmed.to_string());
    normalize(&mut entries);
    let lines = entries
        .iter()
        .map(|entry| {
            serde_json::to_vec(&HistoryRecord {
                text: entry.clone(),
            })
            .map_err(|error| format!("序列化输入历史失败：{error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // 整文件重写走同一把锁：与 TUI 同时写入时不会互相截断。
    SHARD_STORE
        .rewrite_with(&path, move |_| Ok(((), lines)))
        .map_err(|error| format!("写入输入历史失败：{error}"))?;
    Ok(entries)
}

/// 丢掉空串、按内容去重（保留最后一次出现）、裁到上限。
fn normalize(entries: &mut Vec<String>) {
    entries.retain(|entry| !entry.trim().is_empty());
    dedupe_keeping_latest(entries);
    let overflow = entries.len().saturating_sub(DRAFT_HISTORY_LIMIT);
    if overflow > 0 {
        entries.drain(..overflow);
    }
}

/// 相同内容只保留最后一次出现的位置：最近说过的话总在最后。
fn dedupe_keeping_latest(entries: &mut Vec<String>) {
    let mut unique: Vec<String> = Vec::with_capacity(entries.len());
    for entry in entries.drain(..).rev() {
        if unique.iter().any(|stored| stored == &entry) {
            continue;
        }
        unique.push(entry);
    }
    unique.reverse();
    *entries = unique;
}

/// Tauri IPC 命令：读取输入历史（最早在前）。
#[tauri::command]
pub fn input_history_list() -> Result<Vec<String>, String> {
    list_input_history()
}

/// Tauri IPC 命令：记录一条输入历史。
#[tauri::command]
pub fn input_history_append(text: String) -> Result<Vec<String>, String> {
    append_input_history(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_drops_blanks_and_keeps_the_latest_position() {
        let mut entries = vec![
            "第一句".to_string(),
            "  ".to_string(),
            "第二句".to_string(),
            "第一句".to_string(),
        ];
        normalize(&mut entries);
        assert_eq!(entries, vec!["第二句".to_string(), "第一句".to_string()]);
    }

    #[test]
    fn normalization_trims_to_the_shared_limit() {
        let mut entries: Vec<String> = (0..DRAFT_HISTORY_LIMIT + 5)
            .map(|index| format!("第 {index} 条"))
            .collect();
        normalize(&mut entries);
        assert_eq!(entries.len(), DRAFT_HISTORY_LIMIT);
        assert_eq!(entries.first().map(String::as_str), Some("第 5 条"));
    }

    #[test]
    fn record_uses_the_same_field_name_as_the_tui() {
        // 字段名错了，桌面端写出的行 TUI 就解析不到（反之亦然）。
        let line = serde_json::to_string(&HistoryRecord {
            text: "改一下 README".to_string(),
        })
        .expect("记录应可序列化");
        assert_eq!(line, r#"{"text":"改一下 README"}"#);
        let parsed: HistoryRecord =
            serde_json::from_str(r#"{"text":"来自 TUI"}"#).expect("应可解析");
        assert_eq!(parsed.text, "来自 TUI");
    }

    #[test]
    fn history_path_is_the_tui_draft_history_file() {
        let path = history_path().expect("应能解析历史路径");
        assert!(
            path.ends_with("data/runtime/draft_history.jsonl")
                || path.ends_with(r"data\runtime\draft_history.jsonl")
        );
    }
}
