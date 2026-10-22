//! 私有便签文件存储（对齐 Codex history-notes 的 notes 命名空间能力）。
//!
//! 便签是普通文本文件，落在会话目录的 `notes/` 子目录内，即
//! `~/.axon/data/sessions/<session_id>/notes/<虚拟路径>`；单个文件上限
//! 1,000,000 UTF-8 字节。支持任意文本整文件写入、追加、按前缀列举、
//! 1-based 行范围读取与区分大小写的字面量检索。

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axon_domain::{AxonError, AxonResult};

use crate::session_notes::write_file_replacing;

/// 便签存储目录名（相对会话目录）。
pub const NOTES_DIR_NAME: &str = "notes";
/// 单个便签文件的 UTF-8 字节上限（对齐 Codex notes 合同）。
pub const NOTES_MAX_FILE_BYTES: usize = 1_000_000;
/// 便签虚拟路径允许的最大字符数。
pub const NOTES_MAX_PATH_CHARS: usize = 512;
/// 单次列举允许返回的文件数上限。
pub const NOTES_LIST_MAX_RESULTS: usize = 1_000;
/// 检索结果单行允许返回的最大字符数。
pub const NOTES_SEARCH_MAX_LINE_CHARS: usize = 400;
/// 便签目录遍历允许的最大深度。
const NOTES_MAX_DEPTH: usize = 16;
/// 便签为空时写入 checkpoint 的占位事实。
const NOTES_EMPTY_CHECKPOINT_HINT: &str = "- 尚未写入私有便签；换窗前必须先调用 builtin.notes_write_file 或 builtin.notes_append_to_file。";
/// checkpoint 内被截断时的提示。
const NOTES_CHECKPOINT_TRUNCATED_HINT: &str =
    "- 便签内容超出 checkpoint 预算，已截断；细节请回读完整便签。";
/// checkpoint 中回读便签的固定指引。
const NOTES_READ_BACK_HINT: &str =
    "- 换窗后先回读私有便签：builtin.notes_list_files_by_prefix → builtin.notes_read_file。";
/// checkpoint 中按需回查归档历史的固定指引。
const NOTES_HISTORY_HINT: &str = "- 需要工具输出等细节时：builtin.history_list_items → builtin.history_read_item / builtin.history_search_contents。";

/// 便签文件排序字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesFileOrderBy {
    /// 按虚拟路径字典序排序。
    Name,
    /// 按创建时间排序。
    CreatedAt,
    /// 按最近更新时间排序。
    UpdatedAt,
}

/// 便签文件排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesFileOrder {
    /// 升序（最早/最小在前）。
    Ascending,
    /// 降序（最新/最大在前）。
    Descending,
}

/// 便签文件元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesFileEntry {
    /// 相对便签根目录的虚拟路径（统一使用 `/` 分隔）。
    pub path: String,
    /// 文件字节数。
    pub size_bytes: u64,
    /// 创建时间（Unix 毫秒；无法获取时退化为修改时间）。
    pub created_at_ms: i64,
    /// 最近更新时间（Unix 毫秒）。
    pub updated_at_ms: i64,
}

/// 便签写入回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesFileReceipt {
    /// 写入后的文件元信息。
    pub file: NotesFileEntry,
    /// 本次实际写入的 UTF-8 字节数。
    pub written_bytes: usize,
}

/// 便签行范围读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesReadReceipt {
    /// 文件虚拟路径。
    pub path: String,
    /// 实际返回的起始行号（1-based）。
    pub start_line: usize,
    /// 实际返回的结束行号（1-based，含）。
    pub stop_line: usize,
    /// 文件总行数。
    pub total_lines: usize,
    /// 返回的文本。
    pub text: String,
}

/// 单条便签检索命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesLineMatch {
    /// 命中行号（1-based）。
    pub line_number: usize,
    /// 命中行内容（超长时按字符截断）。
    pub text: String,
}

/// 单个文件的检索命中集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesFileMatches {
    /// 文件虚拟路径。
    pub path: String,
    /// 命中行。
    pub matches: Vec<NotesLineMatch>,
}

/// 便签检索结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesSearchReceipt {
    /// 命中的文件（按请求顺序）。
    pub files: Vec<NotesFileMatches>,
    /// 因 `max_files` 限制而未返回的匹配文件数（近似值）。
    pub truncated_files: usize,
}

/// 便签文件存储。
#[derive(Debug, Default, Clone, Copy)]
pub struct NotesStore;

impl NotesStore {
    /// 便签根目录。
    #[must_use]
    pub fn notes_root(session_dir: &Path) -> PathBuf {
        session_dir.join(NOTES_DIR_NAME)
    }

    /// 整文件写入（创建或替换）；超过单文件字节上限时拒绝写入。
    pub fn write_file(
        session_dir: &Path,
        path: &str,
        text: &str,
        now_ms: i64,
    ) -> AxonResult<NotesFileReceipt> {
        let path = normalize_note_path(path)?;
        ensure_within_limit(text.len())?;
        let target = resolve_note_path(session_dir, &path)?;
        write_file_replacing(&target, text.as_bytes(), "私有便签")?;
        let file = entry_for_path(session_dir, &path, now_ms)?;
        Ok(NotesFileReceipt {
            file,
            written_bytes: text.len(),
        })
    }

    /// 追加写入（无文件时创建）；追加后超过单文件字节上限时拒绝写入。
    pub fn append_to_file(
        session_dir: &Path,
        path: &str,
        text: &str,
        now_ms: i64,
    ) -> AxonResult<NotesFileReceipt> {
        let path = normalize_note_path(path)?;
        let target = resolve_note_path(session_dir, &path)?;
        let existing = match std::fs::read(&target) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(AxonError::runtime_failure(format!(
                    "读取私有便签失败: {error:?}"
                )));
            }
        };
        let total_bytes = existing.len().saturating_add(text.len());
        ensure_within_limit(total_bytes)?;
        let mut combined = existing;
        combined.extend_from_slice(text.as_bytes());
        write_file_replacing(&target, &combined, "私有便签")?;
        let file = entry_for_path(session_dir, &path, now_ms)?;
        Ok(NotesFileReceipt {
            file,
            written_bytes: text.len(),
        })
    }

    /// 按 1-based 行范围读取；负值表示从末尾回数。
    pub fn read_file(
        session_dir: &Path,
        path: &str,
        start_line: Option<i64>,
        stop_line: Option<i64>,
    ) -> AxonResult<NotesReadReceipt> {
        let path = normalize_note_path(path)?;
        let target = resolve_note_path(session_dir, &path)?;
        let content = match std::fs::read_to_string(&target) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(AxonError::runtime_failure(format!(
                    "私有便签不存在: {path}"
                )));
            }
            Err(error) => {
                return Err(AxonError::runtime_failure(format!(
                    "读取私有便签失败: {error:?}"
                )));
            }
        };
        let lines = content.lines().collect::<Vec<_>>();
        let total_lines = lines.len();
        let start = resolve_line_index(start_line.unwrap_or(1), total_lines, 1);
        let stop = resolve_line_index(stop_line.unwrap_or(-1), total_lines, total_lines);
        if total_lines == 0 || start > stop || start > total_lines {
            return Ok(NotesReadReceipt {
                path,
                start_line: 0,
                stop_line: 0,
                total_lines,
                text: String::new(),
            });
        }
        let slice = &lines[start - 1..stop.min(total_lines)];
        Ok(NotesReadReceipt {
            path,
            start_line: start,
            stop_line: stop.min(total_lines),
            total_lines,
            text: slice.join("\n"),
        })
    }

    /// 按虚拟路径前缀列举便签文件。
    pub fn list_files_by_prefix(
        session_dir: &Path,
        prefix: Option<&str>,
        max_results: Option<usize>,
        order_by: NotesFileOrderBy,
        order: NotesFileOrder,
    ) -> AxonResult<Vec<NotesFileEntry>> {
        let max_results = max_results.unwrap_or(NOTES_LIST_MAX_RESULTS);
        ensure_result_limit(max_results)?;
        let mut entries = list_entries_by_prefix(session_dir, prefix, order_by, order)?;
        entries.truncate(max_results);
        Ok(entries)
    }

    /// 区分大小写的字面量检索；按文件逐个返回命中行。
    pub fn search_contents(
        session_dir: &Path,
        query: &str,
        max_matches_per_file: Option<usize>,
        max_files: Option<usize>,
        path_prefix: Option<&str>,
        recent_file_first: bool,
    ) -> AxonResult<NotesSearchReceipt> {
        if query.trim().is_empty() {
            return Err(AxonError::runtime_failure("便签检索关键字不能为空或纯空白"));
        }
        let max_matches_per_file = max_matches_per_file.unwrap_or(1);
        if max_matches_per_file == 0 {
            return Err(AxonError::runtime_failure("每个文件的命中行上限必须大于 0"));
        }
        let max_files = max_files.unwrap_or(NOTES_LIST_MAX_RESULTS);
        ensure_result_limit(max_files)?;
        let entries = list_entries_by_prefix(
            session_dir,
            path_prefix,
            if recent_file_first {
                NotesFileOrderBy::CreatedAt
            } else {
                NotesFileOrderBy::Name
            },
            if recent_file_first {
                NotesFileOrder::Descending
            } else {
                NotesFileOrder::Ascending
            },
        )?;
        let mut files = Vec::new();
        let mut truncated_files = 0;
        for entry in entries {
            let target = resolve_note_path(session_dir, &entry.path)?;
            let content = match std::fs::read_to_string(&target) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::InvalidData => continue,
                Err(error) => {
                    return Err(AxonError::runtime_failure(format!(
                        "读取私有便签失败: {error:?}"
                    )));
                }
            };
            let mut matches = Vec::new();
            for (index, line) in content.lines().enumerate() {
                if line.contains(query) {
                    matches.push(NotesLineMatch {
                        line_number: index + 1,
                        text: truncate_match_line(line),
                    });
                    if matches.len() >= max_matches_per_file {
                        break;
                    }
                }
            }
            if matches.is_empty() {
                continue;
            }
            if files.len() >= max_files {
                truncated_files += 1;
                continue;
            }
            files.push(NotesFileMatches {
                path: entry.path,
                matches,
            });
        }
        Ok(NotesSearchReceipt {
            files,
            truncated_files,
        })
    }

    /// 列出全部便签虚拟路径（按更新时间倒序，最新在前）。
    pub fn list_note_paths(session_dir: &Path) -> AxonResult<Vec<String>> {
        let entries = Self::list_files_by_prefix(
            session_dir,
            None,
            Some(NOTES_LIST_MAX_RESULTS),
            NotesFileOrderBy::UpdatedAt,
            NotesFileOrder::Descending,
        )?;
        Ok(entries.into_iter().map(|entry| entry.path).collect())
    }

    /// 渲染 canonical checkpoint 的便签事实区（严格不超过给定字节预算）。
    pub fn render_checkpoint(session_dir: &Path, max_bytes: usize) -> AxonResult<String> {
        let entries = Self::list_files_by_prefix(
            session_dir,
            None,
            Some(NOTES_LIST_MAX_RESULTS),
            NotesFileOrderBy::UpdatedAt,
            NotesFileOrder::Descending,
        )?;
        if entries.is_empty() {
            return Ok(NOTES_EMPTY_CHECKPOINT_HINT.to_owned());
        }
        let mut rendered = String::new();
        let mut truncated = false;
        'files: for entry in entries {
            let header = format!("- 便签 {}（{} 字节）:", entry.path, entry.size_bytes);
            if !push_bounded(&mut rendered, &header, max_bytes) {
                truncated = true;
                break 'files;
            }
            let target = resolve_note_path(session_dir, &entry.path)?;
            let content = match std::fs::read_to_string(&target) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue 'files,
                Err(error) => {
                    return Err(AxonError::runtime_failure(format!(
                        "读取私有便签失败: {error:?}"
                    )));
                }
            };
            for line in content.lines() {
                if !push_bounded(&mut rendered, &format!("  {}", line.trim_end()), max_bytes) {
                    truncated = true;
                    break 'files;
                }
            }
        }
        if truncated {
            push_bounded(&mut rendered, NOTES_CHECKPOINT_TRUNCATED_HINT, max_bytes);
        }
        Ok(rendered)
    }

    /// 渲染 canonical checkpoint 的换窗回读指引区（严格不超过给定字节预算）。
    pub fn render_read_back_hint(session_dir: &Path, max_bytes: usize) -> AxonResult<String> {
        let mut rendered = String::new();
        push_bounded(&mut rendered, NOTES_READ_BACK_HINT, max_bytes);
        push_bounded(&mut rendered, NOTES_HISTORY_HINT, max_bytes);
        for path in Self::list_note_paths(session_dir)? {
            if !push_bounded(&mut rendered, &format!("- 便签文件: {path}"), max_bytes) {
                break;
            }
        }
        Ok(rendered)
    }
}

/// 在预算内追加一行；未能完整追加时返回 false。
fn push_bounded(buffer: &mut String, line: &str, max_bytes: usize) -> bool {
    let separator_bytes = usize::from(!buffer.is_empty());
    if buffer
        .len()
        .saturating_add(separator_bytes)
        .saturating_add(line.len())
        > max_bytes
    {
        return false;
    }
    if separator_bytes == 1 {
        buffer.push('\n');
    }
    buffer.push_str(line);
    true
}

fn ensure_within_limit(total_bytes: usize) -> AxonResult<()> {
    if total_bytes > NOTES_MAX_FILE_BYTES {
        return Err(AxonError::runtime_failure(format!(
            "私有便签文件超过上限：{total_bytes} > {NOTES_MAX_FILE_BYTES} UTF-8 字节；请新建另一个文件继续记录。"
        )));
    }
    Ok(())
}

/// 检查 notes 列举和检索的硬结果上限。
fn ensure_result_limit(value: usize) -> AxonResult<()> {
    if !(1..=NOTES_LIST_MAX_RESULTS).contains(&value) {
        return Err(AxonError::runtime_failure(format!(
            "便签结果数量必须在 1 到 {NOTES_LIST_MAX_RESULTS} 之间"
        )));
    }
    Ok(())
}

/// 按虚拟路径过滤并排序全部便签文件；调用方决定是否截断返回数量。
fn list_entries_by_prefix(
    session_dir: &Path,
    prefix: Option<&str>,
    order_by: NotesFileOrderBy,
    order: NotesFileOrder,
) -> AxonResult<Vec<NotesFileEntry>> {
    let prefix = match prefix {
        Some(prefix) => normalize_note_path_prefix(prefix)?,
        None => String::new(),
    };
    let root = NotesStore::notes_root(session_dir);
    let mut entries = Vec::new();
    if root.exists() {
        collect_entries(&root, &root, 0, &mut entries)?;
    }
    entries.retain(|entry| entry.path.starts_with(&prefix));
    sort_entries(&mut entries, order_by, order);
    Ok(entries)
}

/// 归一化便签虚拟路径：拒绝绝对路径、空组件、`.` 与 `..`、以及 `\` 等本地路径字符。
pub fn normalize_note_path(path: &str) -> AxonResult<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AxonError::runtime_failure("便签路径不能为空"));
    }
    if trimmed.chars().count() > NOTES_MAX_PATH_CHARS {
        return Err(AxonError::runtime_failure(format!(
            "便签路径过长，最多 {NOTES_MAX_PATH_CHARS} 个字符"
        )));
    }
    if trimmed.starts_with('/') || trimmed.contains('\\') || trimmed.contains(':') {
        return Err(AxonError::runtime_failure(
            "便签路径必须是虚拟相对路径，不能包含盘符、前导 `/` 或 `\\`",
        ));
    }
    let mut normalized = Vec::new();
    for component in trimmed.split('/') {
        let component = component.trim();
        if component.is_empty() || component == "." || component == ".." {
            return Err(AxonError::runtime_failure(
                "便签路径不支持空组件、`.` 或 `..`",
            ));
        }
        normalized.push(component);
    }
    let normalized = normalized.join("/");
    if Path::new(&normalized)
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AxonError::runtime_failure("便签路径不合法"));
    }
    Ok(normalized)
}

fn normalize_note_path_prefix(prefix: &str) -> AxonResult<String> {
    let trimmed = prefix.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.ends_with('/') {
        Ok(format!(
            "{}/",
            normalize_note_path(trimmed.trim_end_matches('/'))?
        ))
    } else {
        normalize_note_path(trimmed)
    }
}

fn resolve_note_path(session_dir: &Path, path: &str) -> AxonResult<PathBuf> {
    let normalized = normalize_note_path(path)?;
    Ok(NotesStore::notes_root(session_dir)
        .join(normalized.replace('/', std::path::MAIN_SEPARATOR_STR)))
}

fn resolve_line_index(requested: i64, total_lines: usize, default: usize) -> usize {
    if total_lines == 0 {
        return default;
    }
    let total = i64::try_from(total_lines).unwrap_or(i64::MAX);
    let resolved = if requested < 0 {
        total.saturating_add(requested).saturating_add(1)
    } else {
        requested
    };
    if resolved < 1 {
        return 1;
    }
    usize::try_from(resolved).unwrap_or(usize::MAX)
}

fn truncate_match_line(line: &str) -> String {
    if line.chars().count() <= NOTES_SEARCH_MAX_LINE_CHARS {
        return line.to_owned();
    }
    let mut truncated = line
        .chars()
        .take(NOTES_SEARCH_MAX_LINE_CHARS)
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn entry_for_path(session_dir: &Path, path: &str, now_ms: i64) -> AxonResult<NotesFileEntry> {
    let target = resolve_note_path(session_dir, path)?;
    let metadata = std::fs::metadata(&target).map_err(|error| {
        AxonError::runtime_failure(format!("读取私有便签元信息失败: {error:?}"))
    })?;
    let updated_at_ms = metadata.modified().ok().map_or(now_ms, system_time_to_ms);
    let created_at_ms = metadata
        .created()
        .ok()
        .map_or(updated_at_ms, system_time_to_ms);
    Ok(NotesFileEntry {
        path: path.to_owned(),
        size_bytes: metadata.len(),
        created_at_ms,
        updated_at_ms,
    })
}

fn collect_entries(
    root: &Path,
    directory: &Path,
    depth: usize,
    entries: &mut Vec<NotesFileEntry>,
) -> AxonResult<()> {
    if depth > NOTES_MAX_DEPTH {
        return Ok(());
    }
    let read_dir = std::fs::read_dir(directory)
        .map_err(|error| AxonError::runtime_failure(format!("读取私有便签目录失败: {error:?}")))?;
    for entry_result in read_dir {
        let entry = entry_result.map_err(|error| {
            AxonError::runtime_failure(format!("读取私有便签目录项失败: {error:?}"))
        })?;
        let file_type = entry.file_type().map_err(|error| {
            AxonError::runtime_failure(format!("读取私有便签文件类型失败: {error:?}"))
        })?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_entries(root, &path, depth.saturating_add(1), entries)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let virtual_path = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if virtual_path.is_empty() {
            continue;
        }
        let metadata = entry.metadata().map_err(|error| {
            AxonError::runtime_failure(format!("读取私有便签元信息失败: {error:?}"))
        })?;
        let updated_at_ms = metadata.modified().ok().map_or(0, system_time_to_ms);
        let created_at_ms = metadata
            .created()
            .ok()
            .map_or(updated_at_ms, system_time_to_ms);
        entries.push(NotesFileEntry {
            path: virtual_path,
            size_bytes: metadata.len(),
            created_at_ms,
            updated_at_ms,
        });
    }
    Ok(())
}

fn sort_entries(entries: &mut [NotesFileEntry], order_by: NotesFileOrderBy, order: NotesFileOrder) {
    entries.sort_by(|left, right| {
        let ordering = match order_by {
            NotesFileOrderBy::Name => left.path.cmp(&right.path),
            NotesFileOrderBy::CreatedAt => left
                .created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.path.cmp(&right.path)),
            NotesFileOrderBy::UpdatedAt => left
                .updated_at_ms
                .cmp(&right.updated_at_ms)
                .then_with(|| left.path.cmp(&right.path)),
        };
        match order {
            NotesFileOrder::Ascending => ordering,
            NotesFileOrder::Descending => ordering.reverse(),
        }
    });
}

fn system_time_to_ms(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_millis()).unwrap_or(i64::MAX),
    }
}
