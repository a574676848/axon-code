//! notes / history 命名空间工具执行适配（对齐 Codex history-notes 的 9 个动作）。

use axon_agent_runtime::{
    HISTORY_LIST_ITEMS_DEFAULT_LIMIT, HISTORY_LIST_ITEMS_DEFAULT_MAX_CHARS,
    HISTORY_LIST_WINDOWS_DEFAULT_LIMIT, HISTORY_READ_ITEM_DEFAULT_LIMIT_CHARS, HISTORY_ROLE_VALUES,
    HISTORY_SEARCH_CONTENTS_DEFAULT_LIMIT, HistoryItemFilter, NOTES_LIST_MAX_RESULTS,
    NotesFileOrder, NotesFileOrderBy, ProviderToolCall, SessionNotesRuntime,
};
use serde::Deserialize;

use super::orchestrator_builtin::current_session_directory;
use super::{
    ToolOrchestratorContext, ToolReceipt, completed_tool_receipt, failed_tool_input_receipt,
    failed_tool_receipt, failed_tool_receipt_from_axon_error, now_ms,
};

const NOTES_WRITE_FILE_GUIDANCE: &str =
    "请提供 path 与 text；path 是虚拟便签路径，例如 checkpoint.md。";
const NOTES_APPEND_GUIDANCE: &str =
    "请提供 path 与 text；文件不存在时会自动创建，追加受单文件 1,000,000 字节上限约束。";
const NOTES_READ_GUIDANCE: &str =
    "请提供 path；start_line 与 stop_line 可选，1-based 且负值从末尾回数。";
const NOTES_LIST_GUIDANCE: &str = "prefix 可省略或传 null；max_results、file_order_by、file_order 均可选，max_results 必须为 1 到 1000。";
const NOTES_SEARCH_GUIDANCE: &str = "请提供非空或非空白 query；max_matches_per_file、max_files、path_prefix、recent_file_first 均可选，max_files 必须为 1 到 1000。";
const HISTORY_LIST_WINDOWS_GUIDANCE: &str = "limit 与 recent_first 均可选，limit 必须为正整数。";
const HISTORY_LIST_ITEMS_GUIDANCE: &str = "limit、recent_first、tool_namespace、role、tool_name、window_id、max_chars_per_item 均可选；nullable 筛选字段传 null 表示不限，role 区分大小写。";
const HISTORY_READ_ITEM_GUIDANCE: &str = "请提供 window_id 与 item_id（原样使用历史工具返回的值）；offset_chars 与 limit_chars 可选且 limit_chars 必须为正整数。";
const HISTORY_SEARCH_GUIDANCE: &str = "请提供非空或非空白 query；limit、recent_first、tool_namespace、role、tool_name、window_id 均可选；nullable 筛选字段传 null 表示不限，role 区分大小写。";

/// 执行 notes 命名空间工具。
pub(super) fn execute_notes_tool(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    match bare_tool_name(tool_call.dispatch_name()).as_str() {
        "notes_write_file" => execute_notes_write_file(tool_call, context),
        "notes_append_to_file" => execute_notes_append_to_file(tool_call, context),
        "notes_read_file" => execute_notes_read_file(tool_call, context),
        "notes_list_files_by_prefix" => execute_notes_list_files(tool_call, context),
        "notes_search_contents" => execute_notes_search_contents(tool_call, context),
        other => failed_tool_receipt(tool_call, format!("未知 notes 工具：{other}")),
    }
}

/// 执行 history 命名空间工具。
pub(super) fn execute_history_tool(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    match bare_tool_name(tool_call.dispatch_name()).as_str() {
        "history_list_windows" => execute_history_list_windows(tool_call, context),
        "history_list_items" => execute_history_list_items(tool_call, context),
        "history_read_item" => execute_history_read_item(tool_call, context),
        "history_search_contents" => execute_history_search_contents(tool_call, context),
        other => failed_tool_receipt(tool_call, format!("未知 history 工具：{other}")),
    }
}

fn execute_notes_write_file(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input = match parse_input::<NotesWriteFileInput>(tool_call, NOTES_WRITE_FILE_GUIDANCE) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    match SessionNotesRuntime::write_note_file(
        &session_dir,
        &input.path,
        &input.text,
        unix_now_ms(),
    ) {
        Ok(receipt) => completed_tool_receipt(
            tool_call,
            format!(
                "便签已整文件写入：{}（{} 字节）。",
                receipt.file.path, receipt.file.size_bytes
            ),
            None,
            true,
        ),
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn execute_notes_append_to_file(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input = match parse_input::<NotesWriteFileInput>(tool_call, NOTES_APPEND_GUIDANCE) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    match SessionNotesRuntime::append_note_file(
        &session_dir,
        &input.path,
        &input.text,
        unix_now_ms(),
    ) {
        Ok(receipt) => completed_tool_receipt(
            tool_call,
            format!(
                "便签已追加：{}（新增 {} 字节，当前 {} 字节）。",
                receipt.file.path, receipt.written_bytes, receipt.file.size_bytes
            ),
            None,
            true,
        ),
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn execute_notes_read_file(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input = match parse_input::<NotesReadFileInput>(tool_call, NOTES_READ_GUIDANCE) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    match SessionNotesRuntime::read_note_file(
        &session_dir,
        &input.path,
        input.start_line,
        input.stop_line,
    ) {
        Ok(receipt) => {
            let summary = if receipt.total_lines == 0 {
                format!("便签 {} 当前为空。", receipt.path)
            } else {
                receipt.text
            };
            completed_tool_receipt(tool_call, summary, None, false)
        }
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn execute_notes_list_files(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input = match parse_input::<NotesListFilesInput>(tool_call, NOTES_LIST_GUIDANCE) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let order_by = match input.file_order_by.as_deref() {
        None => NotesFileOrderBy::UpdatedAt,
        Some("name") => NotesFileOrderBy::Name,
        Some("created_at") => NotesFileOrderBy::CreatedAt,
        Some("updated_at") => NotesFileOrderBy::UpdatedAt,
        Some(other) => {
            return failed_tool_input_receipt(
                tool_call,
                format!("file_order_by 只支持 name、created_at、updated_at；收到 {other}。"),
            );
        }
    };
    let order = match input.file_order.as_deref() {
        None => NotesFileOrder::Descending,
        Some("ascending") => NotesFileOrder::Ascending,
        Some("descending") => NotesFileOrder::Descending,
        Some(other) => {
            return failed_tool_input_receipt(
                tool_call,
                format!("file_order 只支持 ascending、descending；收到 {other}。"),
            );
        }
    };
    if input.max_results == Some(0) {
        return failed_tool_input_receipt(tool_call, "max_results 必须大于 0。");
    }
    if input
        .max_results
        .is_some_and(|value| value > NOTES_LIST_MAX_RESULTS)
    {
        return failed_tool_input_receipt(
            tool_call,
            format!("max_results 不能超过 {NOTES_LIST_MAX_RESULTS}。"),
        );
    }
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    match SessionNotesRuntime::list_note_files(
        &session_dir,
        input.prefix.as_deref(),
        input.max_results,
        order_by,
        order,
    ) {
        Ok(entries) if entries.is_empty() => {
            completed_tool_receipt(tool_call, "当前会话暂无便签文件。", None, false)
        }
        Ok(entries) => {
            let summary = entries
                .iter()
                .map(|entry| {
                    format!(
                        "{}\t{} 字节\t更新于 {}",
                        entry.path, entry.size_bytes, entry.updated_at_ms
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            completed_tool_receipt(tool_call, summary, None, false)
        }
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn execute_notes_search_contents(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input = match parse_input::<NotesSearchContentsInput>(tool_call, NOTES_SEARCH_GUIDANCE) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    if input.query.trim().is_empty() {
        return failed_tool_input_receipt(tool_call, "query 不能为空或纯空白。");
    }
    if input.max_matches_per_file == Some(0) || input.max_files == Some(0) {
        return failed_tool_input_receipt(
            tool_call,
            "max_matches_per_file 与 max_files 必须大于 0。",
        );
    }
    if input
        .max_files
        .is_some_and(|value| value > NOTES_LIST_MAX_RESULTS)
    {
        return failed_tool_input_receipt(
            tool_call,
            format!("max_files 不能超过 {NOTES_LIST_MAX_RESULTS}。"),
        );
    }
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    match SessionNotesRuntime::search_note_files(
        &session_dir,
        &input.query,
        input.max_matches_per_file,
        input.max_files,
        input.path_prefix.as_deref(),
        input.recent_file_first.unwrap_or(false),
    ) {
        Ok(receipt) if receipt.files.is_empty() => {
            completed_tool_receipt(tool_call, "未命中任何便签内容。", None, false)
        }
        Ok(receipt) => {
            let mut lines = Vec::new();
            for file in receipt.files {
                for hit in file.matches {
                    lines.push(format!("{}:{}: {}", file.path, hit.line_number, hit.text));
                }
            }
            completed_tool_receipt(tool_call, lines.join("\n"), None, false)
        }
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn execute_history_list_windows(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input =
        match parse_input::<HistoryListWindowsInput>(tool_call, HISTORY_LIST_WINDOWS_GUIDANCE) {
            Ok(input) => input,
            Err(message) => return failed_tool_input_receipt(tool_call, message),
        };
    if input.limit == Some(0) {
        return failed_tool_input_receipt(tool_call, "limit 必须大于 0。");
    }
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    match axon_agent_runtime::history_list_windows(
        &session_dir,
        input.limit.unwrap_or(HISTORY_LIST_WINDOWS_DEFAULT_LIMIT),
        input.recent_first.unwrap_or(false),
    ) {
        Ok(result) if result.windows.is_empty() => {
            completed_tool_receipt(tool_call, "暂无可回查的归档窗口。", None, false)
        }
        Ok(result) => {
            let summary = result
                .windows
                .iter()
                .map(|window| {
                    format!(
                        "{}\twindow#{}\titems={}",
                        window.window_id, window.window_number, window.item_count
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            completed_tool_receipt(tool_call, summary, None, false)
        }
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn execute_history_list_items(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input = match parse_input::<HistoryListItemsInput>(tool_call, HISTORY_LIST_ITEMS_GUIDANCE) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    if input.limit == Some(0) || input.max_chars_per_item == Some(0) {
        return failed_tool_input_receipt(tool_call, "limit 与 max_chars_per_item 必须大于 0。");
    }
    let filter = match history_filter(
        input.window_id,
        input.role,
        input.tool_name,
        input.tool_namespace,
    ) {
        Ok(filter) => filter,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    match axon_agent_runtime::history_list_items(
        &session_dir,
        &filter,
        input.limit.unwrap_or(HISTORY_LIST_ITEMS_DEFAULT_LIMIT),
        input.recent_first.unwrap_or(false),
        input
            .max_chars_per_item
            .unwrap_or(HISTORY_LIST_ITEMS_DEFAULT_MAX_CHARS),
    ) {
        Ok(result) if result.items.is_empty() => {
            completed_tool_receipt(tool_call, "没有符合条件的历史条目。", None, false)
        }
        Ok(result) => {
            let summary = result
                .items
                .iter()
                .map(|item| {
                    format!(
                        "[{}] role={} tool={} ordinal={} chars={}\n{}",
                        item.item_id,
                        item.role,
                        item.tool_name.as_deref().unwrap_or("-"),
                        item.ordinal,
                        item.content_chars,
                        item.truncated_content
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            completed_tool_receipt(tool_call, summary, None, false)
        }
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn execute_history_read_item(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input = match parse_input::<HistoryReadItemInput>(tool_call, HISTORY_READ_ITEM_GUIDANCE) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    if input.limit_chars == Some(0) {
        return failed_tool_input_receipt(tool_call, "limit_chars 必须大于 0。");
    }
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    match axon_agent_runtime::history_read_item(
        &session_dir,
        &input.window_id,
        &input.item_id,
        input.offset_chars.unwrap_or(0),
        input
            .limit_chars
            .unwrap_or(HISTORY_READ_ITEM_DEFAULT_LIMIT_CHARS),
    ) {
        Ok(result) => completed_tool_receipt(tool_call, result.text, None, false),
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn execute_history_search_contents(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input = match parse_input::<HistorySearchContentsInput>(tool_call, HISTORY_SEARCH_GUIDANCE)
    {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    if input.query.trim().is_empty() {
        return failed_tool_input_receipt(tool_call, "query 不能为空或纯空白。");
    }
    if input.limit == Some(0) {
        return failed_tool_input_receipt(tool_call, "limit 必须大于 0。");
    }
    let filter = match history_filter(
        input.window_id,
        input.role,
        input.tool_name,
        input.tool_namespace,
    ) {
        Ok(filter) => filter,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let session_dir = match current_session_directory(context) {
        Ok(session_dir) => session_dir,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    match axon_agent_runtime::history_search_contents(
        &session_dir,
        &input.query,
        &filter,
        input.limit.unwrap_or(HISTORY_SEARCH_CONTENTS_DEFAULT_LIMIT),
        input.recent_first.unwrap_or(false),
    ) {
        Ok(result) if result.matches.is_empty() => {
            completed_tool_receipt(tool_call, "未命中任何归档内容。", None, false)
        }
        Ok(result) => {
            let summary = result
                .matches
                .iter()
                .map(|hit| {
                    format!(
                        "[{}] window={} role={} tool={} matches={}\n{}",
                        hit.item_id,
                        hit.window_id,
                        hit.role,
                        hit.tool_name.as_deref().unwrap_or("-"),
                        hit.match_count,
                        hit.snippet
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            completed_tool_receipt(tool_call, summary, None, false)
        }
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn history_filter(
    window_id: Option<String>,
    role: Option<String>,
    tool_name: Option<String>,
    tool_namespace: Option<String>,
) -> Result<HistoryItemFilter, String> {
    let role = match role {
        None => None,
        Some(role) => {
            if !HISTORY_ROLE_VALUES.iter().any(|allowed| *allowed == role) {
                return Err(format!(
                    "role 只支持 {}，且区分大小写；收到 {role}。",
                    HISTORY_ROLE_VALUES.join("、")
                ));
            }
            Some(role)
        }
    };
    Ok(HistoryItemFilter {
        window_id,
        role,
        tool_name,
        tool_namespace,
    })
}

fn bare_tool_name(tool_name: &str) -> String {
    tool_name
        .strip_prefix("builtin.")
        .unwrap_or(tool_name)
        .to_owned()
}

fn parse_input<T>(tool_call: &ProviderToolCall, guidance: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let Some(value) = tool_call.input.as_json() else {
        return Err(format!("工具入参必须是 JSON 对象。{guidance}"));
    };
    reject_explicit_nulls(value, guidance)?;
    serde_json::from_value(value.clone()).map_err(|error| {
        format!(
            "工具入参不符合 schema：{}。{guidance}",
            error
                .to_string()
                .lines()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        )
    })
}

fn reject_explicit_nulls(value: &serde_json::Value, guidance: &str) -> Result<(), String> {
    let Some(fields) = value.as_object() else {
        return Ok(());
    };
    if fields.iter().any(|(field, value)| {
        value.is_null()
            && !matches!(
                field.as_str(),
                "prefix"
                    | "start_line"
                    | "stop_line"
                    | "path_prefix"
                    | "role"
                    | "tool_name"
                    | "tool_namespace"
                    | "window_id"
            )
    }) {
        return Err(format!("schema 不允许非 nullable 字段传 null。{guidance}"));
    }
    Ok(())
}

fn unix_now_ms() -> i64 {
    i64::try_from(now_ms()).unwrap_or(i64::MAX)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotesWriteFileInput {
    path: String,
    text: String,
    /// 模型自述的调用目的；schema 必填，但只用于界面折叠态展示，执行侧接受并忽略。
    #[serde(default, rename = "intent")]
    _intent: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotesReadFileInput {
    path: String,
    #[serde(default)]
    start_line: Option<i64>,
    #[serde(default)]
    stop_line: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotesListFilesInput {
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    file_order_by: Option<String>,
    #[serde(default)]
    file_order: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotesSearchContentsInput {
    query: String,
    #[serde(default)]
    max_matches_per_file: Option<usize>,
    #[serde(default)]
    max_files: Option<usize>,
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default)]
    recent_file_first: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryListWindowsInput {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    recent_first: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryListItemsInput {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    recent_first: Option<bool>,
    #[serde(default)]
    tool_namespace: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    window_id: Option<String>,
    #[serde(default)]
    max_chars_per_item: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryReadItemInput {
    window_id: String,
    item_id: String,
    #[serde(default)]
    offset_chars: Option<usize>,
    #[serde(default)]
    limit_chars: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistorySearchContentsInput {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    recent_first: Option<bool>,
    #[serde(default)]
    tool_namespace: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    window_id: Option<String>,
}
