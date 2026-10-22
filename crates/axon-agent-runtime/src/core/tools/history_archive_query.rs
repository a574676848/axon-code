//! 归档历史的只读查询引擎（对齐 Codex history 命名空间能力）。
//!
//! 数据来源是窗口轮转写下的归档分片 `archived_history/window_{n}.jsonl`：
//! 每个窗口用 `window_id` 标识，每条消息用 `window_id:message_ordinal` 作为
//! 稳定 `item_id`。所有查询都是只读、按 ordinal 排序的字面量查询。

use std::path::Path;

use axon_domain::{AxonError, AxonResult};

use crate::window_rollover::{ArchivedWindowRecord, WindowRolloverEngine};

/// `history_list_items` 默认返回条目数。
pub const HISTORY_LIST_ITEMS_DEFAULT_LIMIT: usize = 20;
/// `history_list_windows` 默认返回窗口数。
pub const HISTORY_LIST_WINDOWS_DEFAULT_LIMIT: usize = 20;
/// `history_search_contents` 默认返回匹配条目数。
pub const HISTORY_SEARCH_CONTENTS_DEFAULT_LIMIT: usize = 5;
/// `history_list_items` 单条截断内容的默认字符数。
pub const HISTORY_LIST_ITEMS_DEFAULT_MAX_CHARS: usize = 800;
/// `history_read_item` 单次读取的默认字符数。
pub const HISTORY_READ_ITEM_DEFAULT_LIMIT_CHARS: usize = 4_000;
/// `history_search_contents` 单条命中摘要的上下文半径（字符）。
pub const HISTORY_SEARCH_SNIPPET_CONTEXT_CHARS: usize = 400;
/// item id 中窗口 ID 与序号的分隔符。
pub const HISTORY_ITEM_ID_SEPARATOR: char = ':';
/// provider 可见条目尾部的 item id 标记前缀（对齐 Codex 的 `[id: ...]`）。
pub const HISTORY_ITEM_ID_MARKER_PREFIX: &str = "[id: ";
/// MCP 工具命名空间前缀。
const MCP_TOOL_NAMESPACE_PREFIX: &str = "mcp__";
/// MCP 工具命名空间分隔符。
const MCP_TOOL_NAMESPACE_SEPARATOR: &str = "__";

/// 历史条目过滤条件。
#[derive(Debug, Clone, Default)]
pub struct HistoryItemFilter {
    /// 限定窗口 ID。
    pub window_id: Option<String>,
    /// 限定角色（user / assistant / tool / system / developer）。
    pub role: Option<String>,
    /// 限定工具名（匹配该条消息发起的 tool call）。
    pub tool_name: Option<String>,
    /// 限定工具命名空间（匹配该条消息发起的 tool call 的命名空间）。
    pub tool_namespace: Option<String>,
}

/// 窗口摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryWindowSummary {
    /// 窗口 ID。
    pub window_id: String,
    /// 窗口序号（从 1 开始）。
    pub window_number: u64,
    /// 该窗口的归档条目数。
    pub item_count: usize,
    /// 该窗口的归档时间（Unix 毫秒）。
    pub archived_at_ms: i64,
}

/// 窗口列举结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryListWindowsResult {
    /// 窗口摘要（按请求顺序）。
    pub windows: Vec<HistoryWindowSummary>,
    /// 归档中的窗口总数。
    pub total_windows: usize,
}

/// 历史条目摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItemSummary {
    /// 稳定条目 ID（`window_id:message_ordinal`）。
    pub item_id: String,
    /// 所属窗口 ID。
    pub window_id: String,
    /// 所属窗口序号。
    pub window_number: u64,
    /// 条目在窗口中的序号。
    pub ordinal: u64,
    /// 消息角色。
    pub role: String,
    /// 该条目发起的工具名（若有）。
    pub tool_name: Option<String>,
    /// 条目正文总字符数。
    pub content_chars: usize,
    /// 按预算截断后的正文。
    pub truncated_content: String,
}

/// 条目列举结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryListItemsResult {
    /// 条目摘要（按请求顺序）。
    pub items: Vec<HistoryItemSummary>,
    /// 命中的条目总数（未截断）。
    pub total_items: usize,
}

/// 条目读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItemReadResult {
    /// 条目 ID。
    pub item_id: String,
    /// 所属窗口 ID。
    pub window_id: String,
    /// 消息角色。
    pub role: String,
    /// 该条目发起的工具名（若有）。
    pub tool_name: Option<String>,
    /// 本次读取的起始字符偏移。
    pub offset_chars: usize,
    /// 条目正文总字符数。
    pub total_chars: usize,
    /// 返回的正文片段。
    pub text: String,
    /// 是否因预算截断。
    pub truncated: bool,
}

/// 单条检索命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySearchMatch {
    /// 条目 ID。
    pub item_id: String,
    /// 所属窗口 ID。
    pub window_id: String,
    /// 所属窗口序号。
    pub window_number: u64,
    /// 条目序号。
    pub ordinal: u64,
    /// 消息角色。
    pub role: String,
    /// 该条目发起的工具名（若有）。
    pub tool_name: Option<String>,
    /// 该条目内的命中次数。
    pub match_count: usize,
    /// 命中现场摘要。
    pub snippet: String,
}

/// 检索结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySearchContentsResult {
    /// 原始查询字面量。
    pub query: String,
    /// 命中条目。
    pub matches: Vec<HistorySearchMatch>,
    /// 实际扫描的条目数。
    pub scanned_items: usize,
    /// 实际扫描的窗口数。
    pub scanned_windows: usize,
    /// 命中条目总数（未截断）。
    pub total_matches: usize,
}

/// 生成稳定条目 ID。
#[must_use]
pub fn history_item_id(window_id: &str, message_ordinal: u64) -> String {
    format!("{window_id}{HISTORY_ITEM_ID_SEPARATOR}{message_ordinal}")
}

/// 生成 provider 可见条目尾部的 item id 标记。
#[must_use]
pub fn history_item_marker(item_id: &str) -> String {
    format!("{HISTORY_ITEM_ID_MARKER_PREFIX}{item_id}]")
}

/// 返回工具名所属的命名空间；`builtin.read_file` 与 `mcp__server__tool` 都取前缀。
#[must_use]
pub fn tool_name_namespace(tool_name: &str) -> &str {
    let tool_name = tool_name.trim();
    if let Some(rest) = tool_name.strip_prefix(MCP_TOOL_NAMESPACE_PREFIX)
        && let Some((server, _)) = rest.split_once(MCP_TOOL_NAMESPACE_SEPARATOR)
    {
        return &tool_name[..MCP_TOOL_NAMESPACE_PREFIX.len().saturating_add(server.len())];
    }
    tool_name
        .split_once('.')
        .map_or(tool_name, |(namespace, _)| namespace)
}

/// 解析条目 ID；格式不合法时返回 `None`。
#[must_use]
pub fn parse_history_item_id(item_id: &str) -> Option<(String, u64)> {
    let (window_id, ordinal) = item_id.rsplit_once(HISTORY_ITEM_ID_SEPARATOR)?;
    if window_id.trim().is_empty() {
        return None;
    }
    Some((window_id.to_owned(), ordinal.trim().parse::<u64>().ok()?))
}

/// 列举归档窗口。
pub fn history_list_windows(
    session_dir: &Path,
    limit: usize,
    recent_first: bool,
) -> AxonResult<HistoryListWindowsResult> {
    let limit = ensure_positive_limit(limit, "limit")?;
    let shards = WindowRolloverEngine::list_archive_shards(session_dir)?;
    let mut windows = Vec::with_capacity(shards.len());
    for window_number in shards {
        let records = WindowRolloverEngine::read_archive_shard(session_dir, window_number)?;
        let Some(first) = records.first() else {
            continue;
        };
        windows.push(HistoryWindowSummary {
            window_id: first.window_id.clone(),
            window_number,
            item_count: records.len(),
            archived_at_ms: records
                .iter()
                .map(|record| record.archived_at_ms)
                .min()
                .unwrap_or(first.archived_at_ms),
        });
    }
    if recent_first {
        windows.reverse();
    }
    let total_windows = windows.len();
    windows.truncate(limit);
    Ok(HistoryListWindowsResult {
        windows,
        total_windows,
    })
}

/// 列举归档历史条目。
pub fn history_list_items(
    session_dir: &Path,
    filter: &HistoryItemFilter,
    limit: usize,
    recent_first: bool,
    max_chars_per_item: usize,
) -> AxonResult<HistoryListItemsResult> {
    let limit = ensure_positive_limit(limit, "limit")?;
    let max_chars_per_item = ensure_positive_limit(max_chars_per_item, "max_chars_per_item")?;
    let window_numbers = target_window_numbers(session_dir, filter)?;
    let mut items = Vec::new();
    let mut total_items = 0usize;
    visit_records(session_dir, window_numbers, recent_first, &mut |record| {
        if !record_matches_filter(record, filter) {
            return Ok(true);
        }
        total_items = total_items.saturating_add(1);
        if items.len() < limit {
            items.push(summarize_record(record, max_chars_per_item));
        }
        Ok(true)
    })?;
    Ok(HistoryListItemsResult { items, total_items })
}

/// 读取指定条目的一段正文。
pub fn history_read_item(
    session_dir: &Path,
    window_id: &str,
    item_id: &str,
    offset_chars: usize,
    limit_chars: usize,
) -> AxonResult<HistoryItemReadResult> {
    let limit_chars = ensure_positive_limit(limit_chars, "limit_chars")?;
    let (parsed_window_id, ordinal) = parse_history_item_id(item_id)
        .ok_or_else(|| AxonError::runtime_failure("item_id 格式不合法，请使用历史工具返回的 ID"))?;
    if parsed_window_id != window_id {
        return Err(AxonError::runtime_failure(
            "item_id 与 window_id 不匹配，请原样使用历史工具返回的 ID",
        ));
    }
    let window_number = window_number_for_id(session_dir, window_id)?.ok_or_else(|| {
        AxonError::runtime_failure("未知窗口 ID；请使用 history_list_windows 返回的 window_id")
    })?;
    let records = WindowRolloverEngine::read_archive_shard(session_dir, window_number)?;
    let record = records
        .into_iter()
        .find(|record| record.message_ordinal == ordinal)
        .ok_or_else(|| AxonError::runtime_failure("指定的历史条目不存在"))?;
    let text = history_item_text(&record);
    let total_chars = text.chars().count();
    let slice = text
        .chars()
        .skip(offset_chars)
        .take(limit_chars)
        .collect::<String>();
    let truncated = offset_chars.saturating_add(limit_chars) < total_chars;
    Ok(HistoryItemReadResult {
        item_id: item_id.to_owned(),
        window_id: window_id.to_owned(),
        role: record.message.role.clone(),
        tool_name: record_tool_name(&record),
        offset_chars,
        total_chars,
        text: slice,
        truncated,
    })
}

/// 在归档历史中做区分大小写的字面量检索。
pub fn history_search_contents(
    session_dir: &Path,
    query: &str,
    filter: &HistoryItemFilter,
    limit: usize,
    recent_first: bool,
) -> AxonResult<HistorySearchContentsResult> {
    if query.trim().is_empty() {
        return Err(AxonError::runtime_failure("历史检索关键字不能为空或纯空白"));
    }
    let limit = ensure_positive_limit(limit, "limit")?;
    let window_numbers = target_window_numbers(session_dir, filter)?;
    let scanned_windows = window_numbers.len();
    let mut matches = Vec::new();
    let mut scanned_items = 0usize;
    let mut total_matches = 0usize;
    visit_records(session_dir, window_numbers, recent_first, &mut |record| {
        if !record_matches_filter(record, filter) {
            return Ok(true);
        }
        scanned_items = scanned_items.saturating_add(1);
        let text = history_item_text(record);
        let match_count = text.matches(query).count();
        if match_count == 0 {
            return Ok(true);
        }
        total_matches = total_matches.saturating_add(1);
        if matches.len() >= limit {
            return Ok(true);
        }
        let first_match_char = text
            .find(query)
            .map(|byte_index| text[..byte_index].chars().count())
            .unwrap_or_default();
        matches.push(HistorySearchMatch {
            item_id: history_item_id(&record.window_id, record.message_ordinal),
            window_id: record.window_id.clone(),
            window_number: record.window_number,
            ordinal: record.message_ordinal,
            role: record.message.role.clone(),
            tool_name: record_tool_name(record),
            match_count,
            snippet: match_snippet(&text, first_match_char, query.chars().count()),
        });
        Ok(true)
    })?;
    Ok(HistorySearchContentsResult {
        query: query.to_owned(),
        matches,
        scanned_items,
        scanned_windows,
        total_matches,
    })
}

/// 条目完整正文（含工具调用入参与工具执行输出）。
#[must_use]
pub fn history_item_text(record: &ArchivedWindowRecord) -> String {
    let mut text = record.message.content.clone();
    let Some(metadata) = record.message.provider_metadata.as_ref() else {
        return text;
    };
    if let Some(result) = metadata
        .tool_result
        .as_ref()
        .filter(|result| !result.output.trim().is_empty())
    {
        append_item_line(
            &mut text,
            &format!("[Tool Result]: {}", result.output.trim()),
        );
    }
    for call in metadata.tool_calls.as_deref().unwrap_or_default() {
        append_item_line(
            &mut text,
            &format!(
                "[Tool Call: {}]: {}",
                call.name.trim(),
                call.input.canonical_text()
            ),
        );
    }
    text
}

fn append_item_line(target: &mut String, line: &str) {
    if !target.is_empty() {
        target.push('\n');
    }
    target.push_str(line);
}

/// 计算需要扫描的窗口序号（升序）；未知窗口与 Codex 一致地返回空结果。
fn target_window_numbers(session_dir: &Path, filter: &HistoryItemFilter) -> AxonResult<Vec<u64>> {
    match filter.window_id.as_deref() {
        Some(window_id) => Ok(window_number_for_id(session_dir, window_id)?
            .into_iter()
            .collect()),
        None => WindowRolloverEngine::list_archive_shards(session_dir),
    }
}

/// 按请求顺序遍历归档记录；`recent_first` 时使用逆序流式扫描以保持有界内存。
fn visit_records(
    session_dir: &Path,
    window_numbers: Vec<u64>,
    recent_first: bool,
    visit: &mut dyn FnMut(&ArchivedWindowRecord) -> AxonResult<bool>,
) -> AxonResult<()> {
    if recent_first {
        for window_number in window_numbers.into_iter().rev() {
            let mut failure: Option<AxonError> = None;
            WindowRolloverEngine::visit_archive_reverse(
                session_dir,
                window_number,
                &mut |record| match visit(record) {
                    Ok(should_continue) => should_continue,
                    Err(error) => {
                        failure = Some(error);
                        false
                    }
                },
            )?;
            if let Some(error) = failure {
                return Err(error);
            }
        }
        return Ok(());
    }
    for window_number in window_numbers {
        for record in WindowRolloverEngine::read_archive_shard(session_dir, window_number)? {
            if !visit(&record)? {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// 按窗口 ID 定位分片序号。
///
/// 单个分片内的 `window_id` 恒定（写入时由批次校验保证），因此逆序读取首条归档
/// 记录即可判定，无需把每个分片完整读入内存并逐条校验校验和；分片存在损坏记录或
/// 截断尾行时显式失败，不把“无法判定”伪装成未知窗口。
fn window_number_for_id(session_dir: &Path, window_id: &str) -> AxonResult<Option<u64>> {
    for window_number in WindowRolloverEngine::list_archive_shards(session_dir)? {
        let mut matched = false;
        let report = WindowRolloverEngine::visit_archive_reverse(
            session_dir,
            window_number,
            &mut |record| {
                matched = record.window_id == window_id;
                false
            },
        )?;
        if matched {
            return Ok(Some(window_number));
        }
        if report.partial || report.corrupted_records > 0 {
            return Err(AxonError::runtime_failure(format!(
                "归档分片损坏，无法按窗口 ID 定位：window={window_number}"
            )));
        }
    }
    Ok(None)
}

fn record_matches_filter(record: &ArchivedWindowRecord, filter: &HistoryItemFilter) -> bool {
    if let Some(role) = filter.role.as_deref()
        && record.message.role != role
    {
        return false;
    }
    if let Some(tool_name) = filter.tool_name.as_deref() {
        let matched = record_tool_calls(record)
            .is_some_and(|calls| calls.iter().any(|call| call.name == tool_name));
        if !matched {
            return false;
        }
    }
    if let Some(tool_namespace) = filter.tool_namespace.as_deref() {
        let matched = record_tool_calls(record).is_some_and(|calls| {
            calls
                .iter()
                .any(|call| tool_name_namespace(&call.name) == tool_namespace)
        });
        if !matched {
            return false;
        }
    }
    true
}

fn record_tool_calls(
    record: &ArchivedWindowRecord,
) -> Option<&[crate::AgentModelToolCallMetadata]> {
    record
        .message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_calls.as_deref())
}

fn record_tool_name(record: &ArchivedWindowRecord) -> Option<String> {
    record
        .message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_calls.as_deref())
        .and_then(|calls| calls.first())
        .map(|call| call.name.clone())
}

fn summarize_record(
    record: &ArchivedWindowRecord,
    max_chars_per_item: usize,
) -> HistoryItemSummary {
    let text = history_item_text(record);
    let content_chars = text.chars().count();
    let truncated_content = if content_chars <= max_chars_per_item {
        text
    } else {
        let mut bounded = text.chars().take(max_chars_per_item).collect::<String>();
        bounded.push('…');
        bounded
    };
    HistoryItemSummary {
        item_id: history_item_id(&record.window_id, record.message_ordinal),
        window_id: record.window_id.clone(),
        window_number: record.window_number,
        ordinal: record.message_ordinal,
        role: record.message.role.clone(),
        tool_name: record_tool_name(record),
        content_chars,
        truncated_content,
    }
}

fn match_snippet(text: &str, match_char: usize, match_chars: usize) -> String {
    let start = match_char.saturating_sub(HISTORY_SEARCH_SNIPPET_CONTEXT_CHARS);
    let end = match_char
        .saturating_add(match_chars)
        .saturating_add(HISTORY_SEARCH_SNIPPET_CONTEXT_CHARS);
    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.push_str(
        &text
            .chars()
            .skip(start)
            .take(end - start)
            .collect::<String>(),
    );
    if end < text.chars().count() {
        snippet.push('…');
    }
    snippet
}

/// 检查 history 的正整数参数，禁止以 max(1) 静默改写调用者输入。
fn ensure_positive_limit(value: usize, field: &str) -> AxonResult<usize> {
    if value == 0 {
        return Err(AxonError::runtime_failure(format!("{field} 必须大于 0")));
    }
    Ok(value)
}
