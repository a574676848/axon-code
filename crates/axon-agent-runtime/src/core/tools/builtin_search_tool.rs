//! 内置 search tool 的流式、按文件分组分页只读契约。

use std::{path::PathBuf, sync::Arc};

use axon_crypto::Sha256Hex;
use axon_domain::{
    AxonError, AxonResult, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema, ToolSchemaField, ToolSchemaFieldKind,
};

use crate::builtin_search_engine::scan_workspace;
use crate::workspace_walker::WorkspaceWalkCancelToken;

/// 内置 search tool 名称。
pub const SEARCH_TOOL_NAME: &str = "search";
/// search tool schema version。
pub const SEARCH_TOOL_SCHEMA_VERSION: u32 = 4;
/// search tool ID。
pub const SEARCH_TOOL_ID: ToolId = ToolId(12);
/// search tool 必填 query 字段。
pub const SEARCH_TOOL_QUERY_FIELD: &str = "query";
const SEARCH_TOOL_REGEX_MODE_VALUES: &[&str] = &["literal", "regex", "fixed_string"];
/// search tool 只读执行原因。
pub const SEARCH_TOOL_REASON_READ_ONLY: &str = "search_tool:read_only";
/// search tool 不走 shell 原因。
pub const SEARCH_TOOL_REASON_NO_SHELL: &str = "search_tool:no_shell";
/// search tool 拒绝远控触发原因。
pub const SEARCH_TOOL_REASON_REMOTE_TRIGGER_CLOSED: &str = "search_tool:remote_trigger_closed";
/// search tool 结果分页或单文件匹配受限原因。
pub const SEARCH_TOOL_REASON_BOUNDED_RESULTS: &str = "search_tool:bounded_results";
/// search tool 扫描未完整完成原因。
pub const SEARCH_TOOL_REASON_INCOMPLETE: &str = "search_tool:incomplete";
/// search tool 跳过符号链接原因。
pub const SEARCH_TOOL_REASON_SYMLINK_SKIPPED: &str = "search_tool:symlink_skipped";
/// search tool 拒绝扫描根外符号链接原因。
pub const SEARCH_TOOL_REASON_ROOT_ESCAPE_SKIPPED: &str = "search_tool:root_escape_skipped";

const SEARCH_TOOL_QUERY_EMPTY_ERROR: &str = "search query 不能为空";
const SEARCH_TOOL_ROOT_ERROR: &str = "search root 不可访问";
pub(crate) const SEARCH_TOOL_REGEX_ERROR: &str = "search regex 无效";
pub(crate) const SEARCH_TOOL_EMPTY_REGEX_ERROR: &str = "search regex 不能匹配空字符串";

/// search tool 执行输入。
#[derive(Clone)]
pub struct SearchToolExecutionInput {
    /// 查询文本。
    pub query: String,
    /// 工具当前工作目录。
    pub root: PathBuf,
    /// 会话级文件观察存储。
    pub snapshot_session: Option<Arc<crate::WorkspaceFileSession>>,
    /// 字面文件、字面目录或 glob 列表。
    pub paths: Vec<String>,
    /// 搜索模式。
    pub regex_mode: RegexSearchMode,
    /// 是否区分大小写。
    pub case_sensitive: bool,
    /// 是否尊重 Git ignore 规则。
    pub gitignore: bool,
    /// 是否包含隐藏文件。
    pub hidden: bool,
    /// 匹配前上下文行数。
    pub context_before: usize,
    /// 匹配后上下文行数。
    pub context_after: usize,
    /// 有命中文件的 0-based 分页偏移。
    pub file_offset: usize,
    /// 当前页最多返回的有命中文件数。
    pub max_files: Option<usize>,
    /// 单文件最多返回的匹配行数。
    pub max_matches_per_file: Option<usize>,
    /// 协作式取消令牌。
    pub cancel_token: Option<WorkspaceWalkCancelToken>,
    /// 是否远控触发。
    pub remote_requested: bool,
}

/// search tool 查询模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexSearchMode {
    /// 字面搜索。
    Literal,
    /// regex 搜索。
    Regex,
    /// 固定字符串搜索。
    FixedString,
}

/// search tool 执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchToolExecutionStatus {
    /// 已完成。
    Completed,
    /// 被边界阻断。
    Blocked,
}

/// search 返回的匹配行或上下文行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchToolLine {
    /// 行号，1-based。
    pub line_number: usize,
    /// 是否为匹配行。
    pub is_match: bool,
    /// 匹配行中的非重叠字节区间。
    pub match_spans: Vec<(usize, usize)>,
    /// 完整 UTF-8 行文本。
    pub text: String,
}

/// 一个有命中文件的分组结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchToolFile {
    /// 相对 cwd 的展示路径；显式 cwd 外路径使用绝对路径。
    pub path: String,
    /// 模型实际看到且按行号去重后的行。
    pub lines: Vec<SearchToolLine>,
    /// 当前返回的匹配行数。
    pub matched_line_count: usize,
    /// 是否还有未返回的匹配行。
    pub matches_truncated: bool,
    /// 可编辑快照的 Hashline tag；超大文件或无有效快照时为空。
    pub hashline_tag: Option<String>,
}

/// search tool 执行回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchToolExecutionReceipt {
    /// 状态。
    pub status: SearchToolExecutionStatus,
    /// 查询文本。
    pub query: String,
    /// 规范化工作目录。
    pub root: String,
    /// 当前页有命中的文件分组。
    pub files: Vec<SearchToolFile>,
    /// 实际尝试扫描的文件数。
    pub files_scanned: usize,
    /// 当前有命中文件偏移。
    pub file_offset: usize,
    /// 下一页有命中文件偏移。
    pub next_file_offset: Option<usize>,
    /// 不存在的输入路径。
    pub missing_paths: Vec<String>,
    /// 检测为二进制的文件。
    pub binary_paths: Vec<String>,
    /// 无法读取或不是 UTF-8 文本的路径。
    pub unreadable_paths: Vec<String>,
    /// 是否因取消或遍历错误只得到部分结果。
    pub incomplete: bool,
    /// 当前页摘要 hash。
    pub result_sha256: String,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 是否直接执行 shell。
    pub direct_shell_execution_performed: bool,
    /// 是否写入文件。
    pub filesystem_write_performed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// search tool runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct SearchToolRuntime;

impl SearchToolRuntime {
    /// 注册 search tool。
    pub fn register_tool(registry: &mut crate::ToolRegistry) -> AxonResult<ToolDefinition> {
        let tool = search_tool_definition();
        registry.register(tool.clone())?;
        Ok(tool)
    }

    /// 执行只读搜索。
    pub fn execute(
        &self,
        input: SearchToolExecutionInput,
    ) -> AxonResult<SearchToolExecutionReceipt> {
        if input.remote_requested {
            return Ok(blocked_receipt(input));
        }
        if input.query.trim().is_empty() {
            return Err(AxonError::tool_invalid_arguments(
                SEARCH_TOOL_QUERY_EMPTY_ERROR,
            ));
        }
        let root = input
            .root
            .canonicalize()
            .map_err(|_| AxonError::tool_execution_failed(SEARCH_TOOL_ROOT_ERROR))?;
        if !root.is_dir() {
            return Err(AxonError::tool_execution_failed(SEARCH_TOOL_ROOT_ERROR));
        }
        let summary = scan_workspace(&input, &root)?;
        let bounded = summary.next_file_offset.is_some()
            || summary.files.iter().any(|file| file.matches_truncated);
        Ok(SearchToolExecutionReceipt {
            status: SearchToolExecutionStatus::Completed,
            query: input.query,
            root: root.display().to_string(),
            result_sha256: files_sha256(&summary.files),
            reasons: build_reasons(
                bounded,
                summary.incomplete,
                summary.symlinks_skipped,
                summary.root_escapes_skipped,
            ),
            files: summary.files,
            files_scanned: summary.files_scanned,
            file_offset: input.file_offset,
            next_file_offset: summary.next_file_offset,
            missing_paths: summary.missing_paths,
            binary_paths: summary.binary_paths,
            unreadable_paths: summary.unreadable_paths,
            incomplete: summary.incomplete,
            remote_trigger_allowed: false,
            direct_shell_execution_performed: false,
            filesystem_write_performed: false,
        })
    }
}

pub(crate) fn search_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        SEARCH_TOOL_SCHEMA_VERSION,
        [SEARCH_TOOL_QUERY_FIELD, crate::TOOL_INTENT_FIELD],
        vec![
            ToolSchemaField::new(
                SEARCH_TOOL_QUERY_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "要查找的非空文本或正则表达式。",
            ),
            ToolSchemaField::new(
                crate::TOOL_INTENT_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                crate::TOOL_INTENT_FIELD_DESCRIPTION,
            ),
            ToolSchemaField::new(
                "paths",
                ToolSchemaFieldKind::NonBlankStringArray,
                "可选单个路径字符串或路径字符串数组；每项可为字面文件、字面目录或 glob。省略或传空数组时递归搜索 cwd。例如 {\"paths\":\"src\"} 或 {\"paths\":[\"src/**/*.rs\"]}。",
            ),
            ToolSchemaField::new(
                "regex_mode",
                ToolSchemaFieldKind::StringEnum(SEARCH_TOOL_REGEX_MODE_VALUES),
                "查询解释模式；默认 literal。",
            ),
            ToolSchemaField::new(
                "case_sensitive",
                ToolSchemaFieldKind::Boolean,
                "是否区分大小写；默认 false。",
            ),
            ToolSchemaField::new(
                "gitignore",
                ToolSchemaFieldKind::Boolean,
                "是否尊重 .gitignore 与 Git exclude；默认 true。",
            ),
            ToolSchemaField::new(
                "hidden",
                ToolSchemaFieldKind::Boolean,
                "是否包含隐藏文件；默认 false。",
            ),
            ToolSchemaField::new(
                "context_before",
                ToolSchemaFieldKind::Integer,
                "每个匹配前返回的上下文行数；默认 0。",
            ),
            ToolSchemaField::new(
                "context_after",
                ToolSchemaFieldKind::Integer,
                "每个匹配后返回的上下文行数；默认 0。",
            ),
            ToolSchemaField::new(
                "file_offset",
                ToolSchemaFieldKind::Integer,
                "有命中文件的 0-based 分页偏移；默认 0。",
            ),
            ToolSchemaField::new(
                "max_files",
                ToolSchemaFieldKind::Integer,
                "当前页最多返回的有命中文件数，必须为正整数。",
            ),
            ToolSchemaField::new(
                "max_matches_per_file",
                ToolSchemaFieldKind::Integer,
                "单文件最多返回的匹配行数，必须为正整数。",
            ),
        ],
    );
    schema.input_schema["properties"]["context_before"]["minimum"] = serde_json::json!(0);
    schema.input_schema["properties"]["context_after"]["minimum"] = serde_json::json!(0);
    schema.input_schema["properties"]["file_offset"]["minimum"] = serde_json::json!(0);
    schema.input_schema["properties"]["max_files"]["minimum"] = serde_json::json!(1);
    schema.input_schema["properties"]["max_matches_per_file"]["minimum"] = serde_json::json!(1);
    schema.input_schema["properties"]["paths"] = serde_json::json!({
        "anyOf": [
            {
                "type": "string",
                "pattern": r"\S"
            },
            {
                "type": "array",
                "items": {
                    "type": "string",
                    "pattern": r"\S"
                }
            }
        ],
        "description": "可选单个路径字符串或路径字符串数组；每项可为字面文件、字面目录或 glob。省略或传空数组时递归搜索 cwd。",
        "examples": ["src", ["src/**/*.rs"]]
    });
    ToolDefinition {
        id: SEARCH_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: SEARCH_TOOL_NAME.to_owned(),
        description:
            "在代码或文本中查找内容；可限定文件、目录或 glob，并按文件返回带行号的匹配和上下文。intent 是每次调用必填的一句话目的，写法见该字段描述。"
                .to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

fn build_reasons(
    bounded: bool,
    incomplete: bool,
    symlinks_skipped: usize,
    root_escapes_skipped: usize,
) -> Vec<String> {
    let mut reasons = vec![
        SEARCH_TOOL_REASON_READ_ONLY.to_owned(),
        SEARCH_TOOL_REASON_NO_SHELL.to_owned(),
        SEARCH_TOOL_REASON_REMOTE_TRIGGER_CLOSED.to_owned(),
    ];
    if bounded {
        reasons.push(SEARCH_TOOL_REASON_BOUNDED_RESULTS.to_owned());
    }
    if incomplete {
        reasons.push(SEARCH_TOOL_REASON_INCOMPLETE.to_owned());
    }
    if symlinks_skipped > 0 {
        reasons.push(SEARCH_TOOL_REASON_SYMLINK_SKIPPED.to_owned());
    }
    if root_escapes_skipped > 0 {
        reasons.push(SEARCH_TOOL_REASON_ROOT_ESCAPE_SKIPPED.to_owned());
    }
    reasons
}

fn blocked_receipt(input: SearchToolExecutionInput) -> SearchToolExecutionReceipt {
    SearchToolExecutionReceipt {
        status: SearchToolExecutionStatus::Blocked,
        query: input.query,
        root: input.root.display().to_string(),
        files: Vec::new(),
        files_scanned: 0,
        file_offset: input.file_offset,
        next_file_offset: None,
        missing_paths: Vec::new(),
        binary_paths: Vec::new(),
        unreadable_paths: Vec::new(),
        incomplete: false,
        result_sha256: files_sha256(&[]),
        remote_trigger_allowed: false,
        direct_shell_execution_performed: false,
        filesystem_write_performed: false,
        reasons: vec![SEARCH_TOOL_REASON_REMOTE_TRIGGER_CLOSED.to_owned()],
    }
}

fn files_sha256(files: &[SearchToolFile]) -> String {
    let mut hasher = Sha256Hex::new();
    for file in files {
        hasher.update(file.path.as_bytes());
        hasher.update(file.matched_line_count.to_le_bytes());
        hasher.update([u8::from(file.matches_truncated)]);
        if let Some(tag) = &file.hashline_tag {
            hasher.update(tag.as_bytes());
        }
        for line in &file.lines {
            hasher.update(line.line_number.to_le_bytes());
            hasher.update([u8::from(line.is_match)]);
            for (start, end) in &line.match_spans {
                hasher.update(start.to_le_bytes());
                hasher.update(end.to_le_bytes());
            }
            hasher.update(line.text.as_bytes());
        }
    }
    hasher.finalize_hex()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, sync::Arc};

    use super::*;

    fn search_input(root: &Path, query: &str) -> SearchToolExecutionInput {
        SearchToolExecutionInput {
            query: query.to_owned(),
            root: root.to_path_buf(),
            snapshot_session: None,
            paths: Vec::new(),
            regex_mode: RegexSearchMode::Literal,
            case_sensitive: false,
            gitignore: true,
            hidden: false,
            context_before: 0,
            context_after: 0,
            file_offset: 0,
            max_files: None,
            max_matches_per_file: None,
            cancel_token: None,
            remote_requested: false,
        }
    }

    #[test]
    fn search_groups_matches_with_deduplicated_context_and_spans() {
        let root = axon_test_support::test_temp_dir();
        let content = "before\naxon axon\nafter\n";
        fs::write(root.path().join("lib.rs"), content).unwrap();
        let session = Arc::new(crate::WorkspaceFileSession::default());
        let mut input = search_input(root.path(), "axon");
        input.context_before = 1;
        input.context_after = 1;
        input.snapshot_session = Some(Arc::clone(&session));

        let receipt = SearchToolRuntime.execute(input).unwrap();

        assert_eq!(receipt.files.len(), 1);
        let file = &receipt.files[0];
        assert_eq!(file.matched_line_count, 1);
        assert_eq!(file.lines.len(), 3);
        assert_eq!(file.lines[1].match_spans, vec![(0, 4), (5, 9)]);
        assert_eq!(
            file.hashline_tag.as_deref(),
            Some(crate::hashline_snapshot_tag(content).as_str())
        );
        let canonical = root.path().join("lib.rs").canonicalize().unwrap();
        assert!(matches!(
            session.lookup(&canonical, file.hashline_tag.as_deref().unwrap()),
            crate::WorkspaceFileSnapshotLookup::Found(_)
        ));
    }

    #[test]
    fn search_pages_by_matching_file_in_stable_path_order() {
        let root = axon_test_support::test_temp_dir();
        fs::write(root.path().join("a.rs"), "axon\n").unwrap();
        fs::write(root.path().join("b.rs"), "axon\n").unwrap();
        let mut first_input = search_input(root.path(), "axon");
        first_input.max_files = Some(1);

        let first = SearchToolRuntime.execute(first_input).unwrap();
        assert_eq!(first.files[0].path, "a.rs");
        assert_eq!(first.next_file_offset, Some(1));

        let mut second_input = search_input(root.path(), "axon");
        second_input.file_offset = 1;
        second_input.max_files = Some(1);
        let second = SearchToolRuntime.execute(second_input).unwrap();
        assert_eq!(second.files[0].path, "b.rs");
        assert_eq!(second.next_file_offset, None);
    }

    #[test]
    fn search_uses_larger_default_limit_only_for_explicit_file_target() {
        let root = axon_test_support::test_temp_dir();
        fs::create_dir(root.path().join("scope")).unwrap();
        let content = "axon\n".repeat(25);
        fs::write(root.path().join("scope/only.txt"), content).unwrap();

        let mut directory_input = search_input(root.path(), "axon");
        directory_input.paths = vec!["scope".to_owned()];
        let directory = SearchToolRuntime.execute(directory_input).unwrap();

        assert_eq!(directory.files[0].matched_line_count, 20);
        assert!(directory.files[0].matches_truncated);

        let mut file_input = search_input(root.path(), "axon");
        file_input.paths = vec!["scope/only.txt".to_owned()];
        let file = SearchToolRuntime.execute(file_input).unwrap();

        assert_eq!(file.files[0].matched_line_count, 25);
        assert!(!file.files[0].matches_truncated);
    }

    #[test]
    fn search_reports_binary_file_without_folding_it_into_incomplete() {
        let root = axon_test_support::test_temp_dir();
        fs::write(root.path().join("binary.bin"), b"axon\0hidden").unwrap();

        let receipt = SearchToolRuntime
            .execute(search_input(root.path(), "axon"))
            .unwrap();

        assert!(receipt.files.is_empty());
        assert_eq!(receipt.binary_paths, vec!["binary.bin"]);
        assert!(!receipt.incomplete);
    }

    #[test]
    fn search_streams_large_file_without_creating_editable_snapshot() {
        let root = axon_test_support::test_temp_dir();
        let path = root.path().join("large.txt");
        let mut content = b"axon\n".to_vec();
        content.resize(
            crate::workspace_file_session::WORKSPACE_FILE_SNAPSHOT_MAX_BYTES + 1,
            b'x',
        );
        fs::write(&path, content).unwrap();

        let receipt = SearchToolRuntime
            .execute(search_input(root.path(), "axon"))
            .unwrap();

        assert_eq!(receipt.files.len(), 1);
        assert_eq!(receipt.files[0].hashline_tag, None);
    }

    #[test]
    fn search_remote_request_is_blocked_before_filesystem_access() {
        let mut input = search_input(Path::new("missing"), "axon");
        input.remote_requested = true;

        let receipt = SearchToolRuntime.execute(input).unwrap();

        assert_eq!(receipt.status, SearchToolExecutionStatus::Blocked);
        assert!(receipt.files.is_empty());
    }
}
