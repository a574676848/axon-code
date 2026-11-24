//! 内置只读文件工具：`read_file`。
//!
//! 该工具只读、无副作用，不写文件、不走 shell、不允许远控触发；输出分页有界。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use axon_crypto::sha256_hex;
use axon_domain::{
    AxonError, AxonResult, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema, ToolSchemaField, ToolSchemaFieldKind,
};
use axon_output_filter::{
    ExtractedSymbol, OutputFilter,
    lsp_symbol_filter::{LspFilterLevel, TokenSavings},
    regex_fallback::Language,
};

#[path = "builtin_document_reader.rs"]
mod builtin_document_reader;
#[path = "builtin_fs_tools_paging.rs"]
mod builtin_fs_tools_paging;
#[path = "builtin_fs_tools_image.rs"]
pub(crate) mod image;
pub use builtin_document_reader::ReadFileContentKind;
use builtin_document_reader::extract_document;
use builtin_fs_tools_paging::{
    READ_FILE_DEFAULT_LINE_LIMIT, missing_path_error, page_text, read_text_page,
};

#[path = "adaptive_filter.rs"]
mod adaptive_filter;
use crate::WorkspaceFileSession;
use adaptive_filter::SmartFilterStrategy;

/// read_file 工具名称。
pub const READ_FILE_TOOL_NAME: &str = "read_file";
/// read_file 工具 ID。
pub const READ_FILE_TOOL_ID: ToolId = ToolId(14);
/// read_file 工具 schema version。
pub const READ_FILE_TOOL_SCHEMA_VERSION: u32 = 2;
/// read_file 工具必填 path 字段。
pub const READ_FILE_TOOL_PATH_FIELD: &str = "path";
/// read_file 工具可选 filter_level 字段。
pub const READ_FILE_TOOL_FILTER_LEVEL_FIELD: &str = "filter_level";
/// read_file 工具可选 offset 字段。
pub const READ_FILE_TOOL_OFFSET_FIELD: &str = "offset";
/// read_file 工具可选 limit 字段。
pub const READ_FILE_TOOL_LIMIT_FIELD: &str = "limit";

/// 只读 FS 工具只读执行原因。
pub const FS_TOOL_REASON_READ_ONLY: &str = "fs_tool:read_only";
/// 只读 FS 工具不走 shell 原因。
pub const FS_TOOL_REASON_NO_SHELL: &str = "fs_tool:no_shell";
/// 只读 FS 工具拒绝远控触发原因。
pub const FS_TOOL_REASON_REMOTE_TRIGGER_CLOSED: &str = "fs_tool:remote_trigger_closed";
/// 只读 FS 工具结果有界原因。
pub const FS_TOOL_REASON_BOUNDED_RESULTS: &str = "fs_tool:bounded_results";
const FS_TOOL_ROOT_ERROR: &str = "fs_tool root 不可访问";
const FS_TOOL_PATH_NOT_FOUND_ERROR: &str = "fs_tool path 不存在；请先用 builtin.glob 确认完整路径";
const FS_TOOL_PATH_UNAVAILABLE_ERROR: &str = "fs_tool path 不可访问";
const FS_TOOL_NOT_FILE_ERROR: &str = "fs_tool path 不是文件";

/// 只读 FS 工具执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsToolExecutionStatus {
    /// 已完成。
    Completed,
    /// 被边界阻断。
    Blocked,
}

/// read_file 执行输入。
#[derive(Debug, Clone)]
pub struct ReadFileToolExecutionInput {
    /// 访问根目录。
    pub root: PathBuf,
    /// 相对/绝对文件路径。
    pub path: PathBuf,
    /// 会话级文件观察存储；无会话所有者时不生成可编辑快照。
    pub snapshot_session: Option<Arc<WorkspaceFileSession>>,
    /// 起始行号，1-based。
    pub offset: Option<usize>,
    /// 最大返回行数。
    pub limit: Option<usize>,
    /// 是否远控触发。
    pub remote_requested: bool,
    /// 输出过滤级别。
    pub filter_level: Option<LspFilterLevel>,
}

/// read_file 执行回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFileToolExecutionReceipt {
    /// 状态。
    pub status: FsToolExecutionStatus,
    /// 相对路径。
    pub path: String,
    /// 实际返回的内容类型。
    pub content_kind: ReadFileContentKind,
    /// 带原始行号的有界内容页。
    pub preview: String,
    /// 结果是否还有后续内容。
    pub truncated: bool,
    /// 当前页起始行号。
    pub line_start: usize,
    /// 当前页结束行号；空文件为 0。
    pub line_end: usize,
    /// 文件总行数。
    pub total_lines: usize,
    /// 下一页起始行号；到达文件末尾时为空。
    pub next_offset: Option<usize>,
    /// 当前页 UTF-8 字节数。
    pub returned_bytes: usize,
    /// 截断原因。
    pub truncation_reason: Option<String>,
    /// 原始字符数。
    pub original_chars: usize,
    /// 过滤后字符数。
    pub filtered_chars: usize,
    /// 实际应用的过滤级别。
    pub filter_level_applied: Option<LspFilterLevel>,
    /// 提取出的符号。
    pub symbols_extracted: Vec<ExtractedSymbol>,
    /// 本次过滤是否使用 LSP。
    pub lsp_used: bool,
    /// Token 节省统计。
    pub token_savings: TokenSavings,
    /// 内容 hash。
    pub content_sha256: String,
    /// 会话快照标签；未完整观察文件时为空。
    pub snapshot_tag: Option<String>,
    /// 本页真正完整展示的 1-based 行号。
    pub seen_lines: Vec<usize>,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 是否写入文件。
    pub filesystem_write_performed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 只读 FS 工具 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ReadOnlyFsToolRuntime;

impl ReadOnlyFsToolRuntime {
    /// 执行只读文件读取。
    pub fn read_file(
        &self,
        input: ReadFileToolExecutionInput,
    ) -> AxonResult<ReadFileToolExecutionReceipt> {
        let ReadFileToolExecutionInput {
            root,
            path,
            snapshot_session,
            offset,
            limit,
            remote_requested,
            filter_level,
        } = input;
        let offset = offset.unwrap_or(1).max(1);
        let limit = limit.unwrap_or(READ_FILE_DEFAULT_LINE_LIMIT).max(1);
        if remote_requested {
            return Ok(ReadFileToolExecutionReceipt {
                status: FsToolExecutionStatus::Blocked,
                path: path.display().to_string(),
                content_kind: ReadFileContentKind::Text,
                preview: String::new(),
                truncated: false,
                line_start: offset,
                line_end: 0,
                total_lines: 0,
                next_offset: None,
                returned_bytes: 0,
                truncation_reason: None,
                original_chars: 0,
                filtered_chars: 0,
                filter_level_applied: None,
                symbols_extracted: Vec::new(),
                lsp_used: false,
                token_savings: TokenSavings::default(),
                content_sha256: sha256_hex([]),
                snapshot_tag: None,
                seen_lines: Vec::new(),
                remote_trigger_allowed: false,
                filesystem_write_performed: false,
                reasons: vec![FS_TOOL_REASON_REMOTE_TRIGGER_CLOSED.to_owned()],
            });
        }
        let root = canonical_root(&root)?;
        let target = resolve_tool_path(&root, &path)?;
        if !target.is_file() {
            return Err(AxonError::tool_invalid_arguments(FS_TOOL_NOT_FILE_ERROR));
        }
        let filter_level = SmartFilterStrategy::from_env().decide(
            filter_level,
            fs::metadata(&target)
                .map(|metadata| metadata.len())
                .unwrap_or_default(),
        );
        let extracted = extract_document(&target)?;
        let (page, content_kind, filter_level_applied) = if let Some(document) = extracted {
            let mut page = page_text(&document.content, offset, limit)?;
            page.snapshot_content = None;
            page.seen_lines.clear();
            (page, document.kind, None)
        } else if matches!(filter_level, LspFilterLevel::None) {
            (
                read_text_page(&target, offset, limit)?,
                ReadFileContentKind::Text,
                Some(filter_level),
            )
        } else {
            let content = fs::read_to_string(&target)
                .map_err(|_| AxonError::tool_execution_failed(FS_TOOL_PATH_UNAVAILABLE_ERROR))?;
            let filter_result = filter_file_content(&content, &target, filter_level)?;
            let mut page = page_text(&filter_result.filtered_content, offset, limit)?;
            page.content_sha256 = sha256_hex(content.as_bytes());
            page.original_chars = content.chars().count();
            page.snapshot_content = Some(content);
            page.seen_lines.clear();
            page.filter_result = Some(filter_result);
            (page, ReadFileContentKind::Text, Some(filter_level))
        };
        let snapshot_tag = snapshot_session.and_then(|session| {
            page.snapshot_content.as_ref().and_then(|content| {
                session
                    .record_read(&target, content.clone(), page.seen_lines.iter().copied())
                    .map(|snapshot| snapshot.tag().to_owned())
            })
        });
        let seen_lines = page.seen_lines.iter().copied().collect::<Vec<_>>();
        let filter_result = page.filter_result;
        let (symbols_extracted, lsp_used, token_savings) = filter_result.map_or_else(
            || (Vec::new(), false, TokenSavings::default()),
            |result| (result.symbols, result.lsp_used, result.token_savings),
        );
        Ok(ReadFileToolExecutionReceipt {
            status: FsToolExecutionStatus::Completed,
            path: relative_path(&root, &target),
            content_kind,
            preview: page.preview,
            truncated: page.next_offset.is_some(),
            line_start: offset,
            line_end: page.line_end,
            total_lines: page.total_lines,
            next_offset: page.next_offset,
            returned_bytes: page.returned_bytes,
            truncation_reason: page.truncation_reason,
            original_chars: page.original_chars,
            filtered_chars: page.filtered_chars,
            filter_level_applied,
            symbols_extracted,
            lsp_used,
            token_savings,
            content_sha256: page.content_sha256,
            snapshot_tag,
            seen_lines,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            reasons: fs_tool_reasons(page.next_offset.is_some()),
        })
    }
}

/// read_file 工具定义。
pub(crate) fn read_file_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        READ_FILE_TOOL_SCHEMA_VERSION,
        [READ_FILE_TOOL_PATH_FIELD],
        vec![
            ToolSchemaField::new(
                READ_FILE_TOOL_PATH_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "文件路径；相对路径基于 root，也接受绝对路径和父目录路径。",
            ),
            ToolSchemaField::new(
                READ_FILE_TOOL_OFFSET_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选 1-based 起始行；默认从首行开始。",
            ),
            ToolSchemaField::new(
                READ_FILE_TOOL_LIMIT_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选最大返回行数，必须为正整数。",
            ),
            ToolSchemaField::new(
                READ_FILE_TOOL_FILTER_LEVEL_FIELD,
                ToolSchemaFieldKind::StringEnum(&["none", "minimal", "aggressive"]),
                "输出过滤强度；默认 none。",
            ),
        ],
    );
    schema.input_schema["properties"][READ_FILE_TOOL_OFFSET_FIELD]["minimum"] =
        serde_json::json!(1);
    schema.input_schema["properties"][READ_FILE_TOOL_LIMIT_FIELD]["minimum"] = serde_json::json!(1);
    ToolDefinition {
        id: READ_FILE_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: READ_FILE_TOOL_NAME.to_owned(),
        description: "读取一个本地文件。文本与受支持文档返回带原始行号的文本页，内容较长时用 offset 和 limit 分页，返回的行号、标签和 nextOffset 可用于继续读取或精确编辑；PNG/JPEG/WebP/GIF 图片返回图片本身，此时只用 path，offset、limit 与 filter_level 会被忽略。看图片不要去安装图像库、写解码脚本或生成缩略图，同一张图已在本轮读过就不要重复读取。图片会自动归一化为单帧 8-bit sRGB 并按像素与字节预算降采样，回执给出实际尺寸与坐标换算比例；源图上限为 50 MiB、6400 万像素、长边 65535 px，超过时按提示先缩小。支持绝对路径和包含 .. 的路径。".to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::RefOnOverflow,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

fn canonical_root(root: &Path) -> AxonResult<PathBuf> {
    let canonical = root
        .canonicalize()
        .map_err(|_| AxonError::tool_execution_failed(FS_TOOL_ROOT_ERROR))?;
    if !canonical.is_dir() {
        return Err(AxonError::tool_execution_failed(FS_TOOL_ROOT_ERROR));
    }
    Ok(canonical)
}

/// 将已存在的工具路径解析为规范化绝对路径。
pub fn resolve_existing_tool_path(root: &Path, path: &Path) -> AxonResult<PathBuf> {
    let canonical_root = canonical_root(root)?;
    let requested = if path.is_absolute() {
        path.to_path_buf()
    } else {
        canonical_root.join(path)
    };
    requested.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            missing_path_error(&canonical_root, path)
        } else {
            AxonError::tool_execution_failed(FS_TOOL_PATH_UNAVAILABLE_ERROR)
        }
    })
}

fn resolve_tool_path(root: &Path, path: &Path) -> AxonResult<PathBuf> {
    resolve_existing_tool_path(root, path)
}

fn fs_tool_reasons(truncated: bool) -> Vec<String> {
    let mut reasons = vec![
        FS_TOOL_REASON_READ_ONLY.to_owned(),
        FS_TOOL_REASON_NO_SHELL.to_owned(),
        FS_TOOL_REASON_REMOTE_TRIGGER_CLOSED.to_owned(),
    ];
    if truncated {
        reasons.push(FS_TOOL_REASON_BOUNDED_RESULTS.to_owned());
    }
    reasons
}

fn filter_file_content(
    content: &str,
    target: &Path,
    level: LspFilterLevel,
) -> AxonResult<axon_output_filter::lsp_symbol_filter::LspFilterResult> {
    let language = target
        .extension()
        .and_then(|extension| extension.to_str())
        .map_or(Language::Unknown, Language::from_extension);
    OutputFilter
        .filter_code(content, language, level)
        .map_err(|_| AxonError::tool_invalid_definition("read_file 输出过滤失败"))
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
#[path = "builtin_fs_tools_tests.rs"]
mod tests;
