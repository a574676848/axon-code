//! 内置 glob tool 的统一路径发现、排序与分页。

use std::path::PathBuf;

use axon_crypto::Sha256Hex;
use axon_domain::{
    AxonResult, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema,
    ToolSchemaField, ToolSchemaFieldKind,
};

use crate::workspace_walker::{
    WorkspaceWalkCancelToken, WorkspaceWalkDefaultScope, WorkspaceWalkInclude, WorkspaceWalkInput,
    WorkspaceWalker,
};

/// 内置 glob tool 名称。
pub const GLOB_TOOL_NAME: &str = "glob";
/// glob tool ID。
pub const GLOB_TOOL_ID: ToolId = ToolId(15);
/// glob schema 版本。
pub const GLOB_TOOL_SCHEMA_VERSION: u32 = 3;
const GLOB_INCLUDE_VALUES: &[&str] = &["files", "directories", "all"];
const GLOB_INCLUDE_ARRAY_MAX_ITEMS: usize = 2;
const GLOB_DEFAULT_LIMIT: usize = 200;
const GLOB_REASON_READ_ONLY: &str = "glob_tool:read_only";
const GLOB_REASON_REMOTE_TRIGGER_CLOSED: &str = "glob_tool:remote_trigger_closed";
const GLOB_RECURSIVE_PREFIX: &str = "**/";

/// glob 条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobInclude {
    /// 只返回文件。
    Files,
    /// 只返回目录。
    Directories,
    /// 返回文件和目录。
    All,
}

/// glob 执行输入。
#[derive(Clone)]
pub struct GlobToolExecutionInput {
    /// 工具 cwd。
    pub root: PathBuf,
    /// 字面文件、字面目录或 glob 列表。
    pub paths: Vec<String>,
    /// 是否包含隐藏条目。
    pub hidden: bool,
    /// 是否尊重 Git ignore 规则。
    pub gitignore: bool,
    /// 条目类型过滤。
    pub include: GlobInclude,
    /// 0-based 分页偏移。
    pub offset: usize,
    /// 页大小。
    pub limit: Option<usize>,
    /// 是否远控触发。
    pub remote_requested: bool,
    /// 协作式取消令牌。
    pub cancel_token: Option<WorkspaceWalkCancelToken>,
}

/// glob 执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobToolExecutionStatus {
    /// 已完成。
    Completed,
    /// 被边界阻断。
    Blocked,
}

/// glob 返回条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobToolEntry {
    /// 展示路径；目录以 `/` 结尾。
    pub path: String,
    /// 是否为目录。
    pub is_dir: bool,
    /// 最后修改时间，Unix 毫秒。
    pub modified_at_ms: u128,
}

/// glob 执行回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobToolExecutionReceipt {
    /// 执行状态。
    pub status: GlobToolExecutionStatus,
    /// 规范化 cwd。
    pub root: String,
    /// 当前页条目。
    pub entries: Vec<GlobToolEntry>,
    /// 当前 0-based 偏移。
    pub offset: usize,
    /// 下一页偏移。
    pub next_offset: Option<usize>,
    /// 是否还有后续页。
    pub truncated: bool,
    /// 不存在的输入路径。
    pub missing_paths: Vec<String>,
    /// 无法读取的路径。
    pub unreadable_paths: Vec<String>,
    /// 是否因取消或遍历错误只返回部分结果。
    pub incomplete: bool,
    /// 当前页内容哈希。
    pub result_sha256: String,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// glob tool runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct GlobToolRuntime;

impl GlobToolRuntime {
    /// 执行路径发现与分页。
    pub fn execute(&self, input: GlobToolExecutionInput) -> AxonResult<GlobToolExecutionReceipt> {
        if input.remote_requested {
            return Ok(blocked_receipt(input));
        }
        let root = input.root.canonicalize().map_err(|_| {
            axon_domain::AxonError::tool_invalid_arguments("builtin.glob cwd 不可访问")
        })?;
        let limit = input.limit.unwrap_or(GLOB_DEFAULT_LIMIT).max(1);
        let report = WorkspaceWalker::walk(WorkspaceWalkInput {
            root: root.clone(),
            paths: normalize_glob_paths(input.paths),
            hidden: input.hidden,
            gitignore: input.gitignore,
            include: match input.include {
                GlobInclude::Files => WorkspaceWalkInclude::Files,
                GlobInclude::Directories => WorkspaceWalkInclude::Directories,
                GlobInclude::All => WorkspaceWalkInclude::All,
            },
            default_scope: WorkspaceWalkDefaultScope::DirectChildren,
            cancel_token: input.cancel_token,
        })?;
        let mut entries = report
            .entries
            .into_iter()
            .map(|entry| GlobToolEntry {
                path: if entry.is_dir {
                    format!("{}/", entry.display_path.trim_end_matches('/'))
                } else {
                    entry.display_path
                },
                is_dir: entry.is_dir,
                modified_at_ms: entry.modified_at_ms,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .modified_at_ms
                .cmp(&left.modified_at_ms)
                .then_with(|| left.path.cmp(&right.path))
        });
        let total_entries = entries.len();
        let start = input.offset.min(total_entries);
        let page = entries
            .into_iter()
            .skip(start)
            .take(limit)
            .collect::<Vec<_>>();
        let next_offset =
            (start.saturating_add(page.len()) < total_entries).then_some(start + page.len());
        let truncated = next_offset.is_some();
        Ok(GlobToolExecutionReceipt {
            status: GlobToolExecutionStatus::Completed,
            root: root.display().to_string(),
            result_sha256: entries_sha256(&page),
            entries: page,
            offset: input.offset,
            next_offset,
            truncated,
            missing_paths: report.missing_paths,
            unreadable_paths: report.unreadable_paths,
            incomplete: report.incomplete,
            remote_trigger_allowed: false,
            reasons: vec![GLOB_REASON_READ_ONLY.to_owned()],
        })
    }
}

/// 生成 glob 工具定义。
pub(crate) fn glob_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        GLOB_TOOL_SCHEMA_VERSION,
        [crate::TOOL_INTENT_FIELD],
        vec![
            ToolSchemaField::new(
                crate::TOOL_INTENT_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                crate::TOOL_INTENT_FIELD_DESCRIPTION,
            ),
            ToolSchemaField::new(
                "paths",
                ToolSchemaFieldKind::NonBlankStringArray,
                "可选单个路径字符串或路径字符串数组；每项可为字面文件、字面目录或 glob。省略或传空数组时列出 cwd 直接子项。",
            ),
            ToolSchemaField::new(
                "hidden",
                ToolSchemaFieldKind::Boolean,
                "是否包含隐藏条目；默认 false。",
            ),
            ToolSchemaField::new(
                "gitignore",
                ToolSchemaFieldKind::Boolean,
                "是否尊重 .gitignore 与 Git exclude；默认 true。",
            ),
            ToolSchemaField::new(
                "include",
                ToolSchemaFieldKind::StringEnum(GLOB_INCLUDE_VALUES),
                "返回类型：files、directories 或 all；默认 all。兼容使用 [files, directories] 表示同时返回两类条目。",
            ),
            ToolSchemaField::new(
                "offset",
                ToolSchemaFieldKind::Integer,
                "0-based 分页偏移；默认 0。",
            ),
            ToolSchemaField::new(
                "limit",
                ToolSchemaFieldKind::Integer,
                "页大小，必须为正整数。",
            ),
        ],
    );
    schema.input_schema["properties"]["include"] = serde_json::json!({
        "description": "返回类型。单值示例：\"files\"、\"directories\" 或 \"all\"；需要同时返回文件和目录时使用数组 [\"files\", \"directories\"]。省略时默认为 \"all\"。",
        "default": "all",
        "examples": ["files", "directories", "all", ["files", "directories"]],
        "oneOf": [
            {
                "type": "string",
                "enum": GLOB_INCLUDE_VALUES
            },
            {
                "type": "array",
                "items": {
                    "type": "string",
                    "enum": GLOB_INCLUDE_VALUES
                },
                "minItems": 1,
                "maxItems": GLOB_INCLUDE_ARRAY_MAX_ITEMS,
                "uniqueItems": true
            }
        ]
    });
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
        "description": "可选单个路径字符串或路径字符串数组；每项可为字面文件、字面目录或 glob。省略或传空数组时列出 cwd 直接子项。",
        "examples": ["src", ["src/**/*.rs"]]
    });
    schema.input_schema["properties"]["limit"]["minimum"] = serde_json::json!(1);
    ToolDefinition {
        id: GLOB_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: GLOB_TOOL_NAME.to_owned(),
        description:
            "不知道精确文件位置时，按路径或 glob 查找文件和目录；省略 paths 可查看当前目录。intent 是每次调用必填的一句话目的，写法见该字段描述。"
                .to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

fn blocked_receipt(input: GlobToolExecutionInput) -> GlobToolExecutionReceipt {
    GlobToolExecutionReceipt {
        status: GlobToolExecutionStatus::Blocked,
        root: input.root.display().to_string(),
        entries: Vec::new(),
        offset: input.offset,
        next_offset: None,
        truncated: false,
        missing_paths: Vec::new(),
        unreadable_paths: Vec::new(),
        incomplete: false,
        result_sha256: entries_sha256(&[]),
        remote_trigger_allowed: false,
        reasons: vec![GLOB_REASON_REMOTE_TRIGGER_CLOSED.to_owned()],
    }
}

fn entries_sha256(entries: &[GlobToolEntry]) -> String {
    let mut hasher = Sha256Hex::new();
    for entry in entries {
        hasher.update(entry.path.as_bytes());
        hasher.update([u8::from(entry.is_dir)]);
        hasher.update(entry.modified_at_ms.to_le_bytes());
    }
    hasher.finalize_hex()
}

fn normalize_glob_paths(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .map(|path| {
            let normalized = path.trim().replace('\\', "/");
            if normalized.is_empty()
                || normalized.contains('/')
                || normalized.starts_with(GLOB_RECURSIVE_PREFIX)
                || !normalized.contains(['*', '?', '[', '{'])
                || is_exact_brace_union(&normalized)
            {
                normalized
            } else {
                format!("{GLOB_RECURSIVE_PREFIX}{normalized}")
            }
        })
        .collect()
}

fn is_exact_brace_union(pattern: &str) -> bool {
    if !(pattern.starts_with('{') && pattern.ends_with('}')) {
        return false;
    }
    let inner = &pattern[1..pattern.len() - 1];
    !inner.is_empty()
        && !inner
            .chars()
            .any(|character| matches!(character, '*' | '?' | '[' | ']' | '{' | '}'))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn glob_literal_directory_is_direct_and_recursive_pattern_is_recursive() {
        let root = axon_test_support::test_temp_dir();
        fs::create_dir_all(root.path().join("src/nested")).unwrap();
        fs::write(root.path().join("src/lib.rs"), "").unwrap();
        fs::write(root.path().join("src/nested/mod.rs"), "").unwrap();

        let direct = GlobToolRuntime
            .execute(input(root.path(), vec!["src"]))
            .unwrap();
        let recursive = GlobToolRuntime
            .execute(input(root.path(), vec!["src/**/*.rs"]))
            .unwrap();

        assert!(
            direct
                .entries
                .iter()
                .any(|entry| entry.path == "src/lib.rs")
        );
        assert!(
            !direct
                .entries
                .iter()
                .any(|entry| entry.path.ends_with("mod.rs"))
        );
        assert_eq!(recursive.entries.len(), 2);
    }

    #[test]
    fn glob_paginates_after_global_deduplication() {
        let root = axon_test_support::test_temp_dir();
        fs::write(root.path().join("a.rs"), "").unwrap();
        fs::write(root.path().join("b.rs"), "").unwrap();
        let mut request = input(root.path(), vec!["*.rs", "a.rs"]);
        request.limit = Some(1);

        let first = GlobToolRuntime.execute(request).unwrap();

        assert_eq!(first.entries.len(), 1);
        assert_eq!(first.next_offset, Some(1));
    }

    #[test]
    fn simple_glob_patterns_are_recursive_by_default() {
        let root = axon_test_support::test_temp_dir();
        fs::create_dir_all(root.path().join("nested/deeper")).unwrap();
        fs::write(root.path().join("top.rs"), "").unwrap();
        fs::write(root.path().join("nested/deeper/leaf.rs"), "").unwrap();

        let report = GlobToolRuntime
            .execute(input(root.path(), vec!["*.rs"]))
            .unwrap();

        assert_eq!(
            report
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["nested/deeper/leaf.rs", "top.rs"]
        );
    }

    fn input(root: &std::path::Path, paths: Vec<&str>) -> GlobToolExecutionInput {
        GlobToolExecutionInput {
            root: root.to_path_buf(),
            paths: paths.into_iter().map(str::to_owned).collect(),
            hidden: false,
            gitignore: true,
            include: GlobInclude::All,
            offset: 0,
            limit: None,
            remote_requested: false,
            cancel_token: None,
        }
    }
}
