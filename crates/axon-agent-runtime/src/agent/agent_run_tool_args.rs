//! Headless run 内置工具的 provider 参数解析合同。
//!
//! 本模块只解析 provider 传入的 JSON arguments，并产出 runtime/CLI 可复用的 typed
//! input。真实文件系统、shell、审批、权限与 LSP 副作用仍由外层 adapter 执行。

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde_json::Value;

use axon_domain::{AxonError, ToolCallInput, ToolError, ToolInput};
use axon_output_filter::lsp_symbol_filter::LspFilterLevel;

use crate::{
    DEFAULT_SHELL_YIELD_TIME_MS, DEFAULT_WRITE_STDIN_YIELD_TIME_MS, GlobInclude,
    GlobToolExecutionInput, ReadFileToolExecutionInput, RegexSearchMode, SearchToolExecutionInput,
    ShellCommandToolRuntime, WriteStdinExecutionInput, job_runtime::JobId,
};
#[path = "agent_run_tool_args_edit_normalize.rs"]
mod agent_run_tool_args_edit_normalize;
use agent_run_tool_args_edit_normalize::normalize_pathdiff_envelope;

const APPLY_PATCH_BEGIN_MARKER: &str = "*** Begin Patch";
const APPLY_PATCH_END_MARKER: &str = "*** End Patch";
const DEFAULT_AGENT_SHELL_OUTPUT_TOKENS: usize = 2_000;

/// edit 未提供 summary 时的默认摘要。
pub const AGENT_RUN_EDIT_DEFAULT_SUMMARY: &str = "编辑文件";

/// shell_command provider 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunShellToolArgs {
    /// 规范化后的当前默认 shell 非交互命令。
    pub command: Vec<String>,
    /// 已与 adapter 工作区合并的有效 cwd。
    pub cwd: PathBuf,
    /// provider 请求的可选超时毫秒数；缺省时不设置终止 deadline。
    pub timeout_ms: Option<u64>,
    /// 首次等待进程输出的时长。
    pub yield_time_ms: u64,
    /// 是否分配 PTY。
    pub tty: bool,
    /// 是否转为后台作业；与 `tty` 硬互斥。
    pub run_in_background: bool,
    /// 本次回执的可选输出 token 预算。
    pub max_output_tokens: Option<usize>,
}

/// edit provider 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunEditToolArgs {
    /// Hashline 或完整 Codex patch 输入文本。
    pub input: String,
    /// provider-visible 摘要。
    pub summary: String,
}

/// 解析 builtin.search provider arguments。
pub fn agent_run_search_tool_input_from_provider_args(
    cwd: &Path,
    arguments_json: &str,
) -> Result<SearchToolExecutionInput, String> {
    let value = parse_arguments(arguments_json, "builtin.search")?;
    let query = required_string(&value, "query", "builtin.search")?;
    if query.trim().is_empty() {
        return Err("builtin.search query 不能为空".to_owned());
    }
    Ok(SearchToolExecutionInput {
        query,
        root: cwd.to_path_buf(),
        snapshot_session: None,
        paths: optional_string_array(&value, "paths", "builtin.search")?,
        regex_mode: optional_regex_mode(&value)?,
        case_sensitive: optional_bool(&value, "case_sensitive", "builtin.search")?.unwrap_or(false),
        gitignore: optional_bool(&value, "gitignore", "builtin.search")?.unwrap_or(true),
        hidden: optional_bool(&value, "hidden", "builtin.search")?.unwrap_or(false),
        context_before: optional_usize(&value, "context_before", "builtin.search")?.unwrap_or(0),
        context_after: optional_usize(&value, "context_after", "builtin.search")?.unwrap_or(0),
        file_offset: optional_usize(&value, "file_offset", "builtin.search")?.unwrap_or(0),
        max_files: optional_positive_usize(&value, "max_files", "builtin.search")?,
        max_matches_per_file: optional_positive_usize(
            &value,
            "max_matches_per_file",
            "builtin.search",
        )?,
        cancel_token: None,
        remote_requested: false,
    })
}

/// 解析 builtin.invoke_skill provider arguments。
pub fn agent_run_invoke_skill_tool_input_from_provider_args(
    arguments_json: &str,
) -> Result<crate::InvokeSkillInput, String> {
    let value = parse_arguments(arguments_json, "builtin.invoke_skill")?;
    let object = value
        .as_object()
        .ok_or_else(|| "builtin.invoke_skill arguments 必须是 JSON 对象".to_owned())?;
    let skill_name = object
        .get("skill_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "builtin.invoke_skill arguments 缺少 skill_name".to_owned())?;
    if object.len() != 1 {
        return Err("builtin.invoke_skill 参数包含未知字段".to_owned());
    }
    crate::parse_invoke_skill_input(&ToolInput {
        fields: vec![skill_name.to_owned()],
    })
    .map_err(|error| format!("builtin.invoke_skill 参数无效: {error:?}"))
}

/// 解析 builtin.read_file provider arguments。
pub fn agent_run_read_file_tool_input_from_provider_args(
    cwd: &Path,
    arguments_json: &str,
) -> Result<ReadFileToolExecutionInput, String> {
    let value = parse_arguments(arguments_json, "builtin.read_file")?;
    let path = required_tool_path(&value, "path", "builtin.read_file")?;
    let filter_level = optional_filter_level(&value)?;
    let offset = optional_positive_usize(&value, "offset", "builtin.read_file")?;
    let limit = optional_positive_usize(&value, "limit", "builtin.read_file")?;
    Ok(ReadFileToolExecutionInput {
        root: cwd.to_path_buf(),
        path,
        snapshot_session: None,
        offset,
        limit,
        remote_requested: false,
        filter_level,
    })
}

/// 解析 builtin.glob provider arguments。
pub fn agent_run_glob_tool_input_from_provider_args(
    cwd: &Path,
    arguments_json: &str,
) -> Result<GlobToolExecutionInput, String> {
    let value = parse_arguments(arguments_json, "builtin.glob")?;
    Ok(GlobToolExecutionInput {
        root: cwd.to_path_buf(),
        paths: optional_string_array(&value, "paths", "builtin.glob")?,
        hidden: optional_bool(&value, "hidden", "builtin.glob")?.unwrap_or(false),
        gitignore: optional_bool(&value, "gitignore", "builtin.glob")?.unwrap_or(true),
        include: optional_glob_include(&value)?,
        offset: optional_usize(&value, "offset", "builtin.glob")?.unwrap_or(0),
        limit: optional_positive_usize(&value, "limit", "builtin.glob")?,
        remote_requested: false,
        cancel_token: None,
    })
}

/// 解析 builtin.shell_command provider arguments。
pub fn agent_run_shell_tool_args_from_provider_input(
    input: &ToolCallInput,
    base_cwd: &Path,
) -> Result<AgentRunShellToolArgs, String> {
    let value = input
        .as_json()
        .ok_or_else(|| "builtin.shell_command 需要 JSON 参数".to_owned())?;
    let raw_script = required_non_empty_string(value, "command", "builtin.shell_command")?;
    let (command_cwd, command) =
        crate::shell_command_wrapper_unwrapper::extract_windows_cd_prefix(&raw_script);
    let script = crate::shell_command_static_analyzer::unwrap_outer_shell_wrapper(&command);
    let cwd = if value.get("workdir").is_none() {
        command_cwd.map_or_else(
            || Ok(base_cwd.to_path_buf()),
            |path| shell_workdir_from_raw(&path, base_cwd),
        )?
    } else {
        optional_shell_workdir(value, base_cwd)?
    };
    let timeout_ms = optional_u64(value, "timeout_ms", "builtin.shell_command")?;
    if timeout_ms.is_some_and(|timeout| timeout < crate::runtime_limits::MIN_SHELL_TIMEOUT_MS) {
        return Err(format!(
            "builtin.shell_command timeout_ms 必须不小于 {}",
            crate::runtime_limits::MIN_SHELL_TIMEOUT_MS
        ));
    }
    let tty = optional_bool(value, "tty", "builtin.shell_command")?.unwrap_or(false);
    let run_in_background = optional_bool(
        value,
        crate::SHELL_COMMAND_TOOL_RUN_IN_BACKGROUND_FIELD,
        "builtin.shell_command",
    )?
    .unwrap_or(false);
    let yield_time_ms = optional_u64(value, "yield_time_ms", "builtin.shell_command")?
        .unwrap_or(if tty { 0 } else { DEFAULT_SHELL_YIELD_TIME_MS });
    let max_output_tokens = optional_usize(value, "max_output_tokens", "builtin.shell_command")?
        .or(Some(DEFAULT_AGENT_SHELL_OUTPUT_TOKENS));
    let command = ShellCommandToolRuntime::command_from_script_at_cwd(&script, &cwd, tty)
        .map_err(shell_command_argument_error)?;
    Ok(AgentRunShellToolArgs {
        command,
        cwd,
        timeout_ms,
        yield_time_ms,
        tty,
        run_in_background,
        max_output_tokens,
    })
}

/// 后台作业工具 provider 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunJobToolArgs {
    /// `shell_command` 以 `run_in_background: true` 启动后返回的作业 ID。
    pub job_id: JobId,
    /// 本次读取的等待窗口毫秒数；`job_kill` 忽略该字段。
    pub yield_time_ms: Option<u64>,
    /// 本次回执的输出 token 预算；`job_kill` 忽略该字段。
    pub max_output_tokens: Option<usize>,
}

/// 解析 builtin.job_output / builtin.job_kill provider arguments。
///
/// 作业 ID 是十进制字符串，与 UUID 形态的 `session_id` 刻意不同形：解析失败即拒绝，
/// 不做「猜一个 ID」的兜底：猜出来的 ID 只会得到一个更含糊的回执。
pub fn agent_run_job_tool_args_from_provider_input(
    input: &ToolCallInput,
    tool_name: &str,
) -> Result<AgentRunJobToolArgs, String> {
    let value = input
        .as_json()
        .ok_or_else(|| format!("{tool_name} 需要 JSON 参数"))?;
    let raw_job_id = required_non_empty_string(value, crate::JOB_TOOL_JOB_ID_FIELD, tool_name)?;
    let job_id = raw_job_id
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .map(JobId)
        .ok_or_else(|| {
            format!(
                "{tool_name} {} 必须是 shell_command 回执中逐字复制的作业 ID（十进制正整数）",
                crate::JOB_TOOL_JOB_ID_FIELD
            )
        })?;
    Ok(AgentRunJobToolArgs {
        job_id,
        yield_time_ms: optional_u64(value, crate::JOB_TOOL_YIELD_TIME_FIELD, tool_name)?,
        max_output_tokens: optional_usize(
            value,
            crate::JOB_TOOL_MAX_OUTPUT_TOKENS_FIELD,
            tool_name,
        )?,
    })
}

/// 解析 builtin.write_stdin provider arguments。
pub fn agent_run_write_stdin_tool_input_from_provider_args(
    arguments_json: &str,
    owner_id: &str,
) -> Result<WriteStdinExecutionInput, String> {
    let value = parse_arguments(arguments_json, "builtin.write_stdin")?;
    let session_id = required_non_empty_string(&value, "session_id", "builtin.write_stdin")?;
    let chars = optional_string(&value, "chars", "builtin.write_stdin")?;
    let ctrl_c = optional_bool(&value, "ctrl_c", "builtin.write_stdin")?.unwrap_or(false);
    if chars.is_some() && ctrl_c {
        return Err("builtin.write_stdin chars 与 ctrl_c 不能同时提供".to_owned());
    }
    let yield_time_ms = optional_u64(&value, "yield_time_ms", "builtin.write_stdin")?
        .unwrap_or(DEFAULT_WRITE_STDIN_YIELD_TIME_MS);
    let max_output_tokens = optional_usize(&value, "max_output_tokens", "builtin.write_stdin")?
        .or(Some(DEFAULT_AGENT_SHELL_OUTPUT_TOKENS));
    Ok(WriteStdinExecutionInput {
        owner_id: owner_id.to_owned(),
        session_id,
        chars,
        ctrl_c,
        yield_time_ms,
        max_output_tokens,
        remote_requested: false,
    })
}

/// 解析 builtin.edit provider 输入。
pub fn agent_run_edit_tool_args_from_provider_input(
    input: &ToolCallInput,
) -> Result<AgentRunEditToolArgs, String> {
    match input {
        ToolCallInput::Text(value) if !value.trim().is_empty() => Ok(AgentRunEditToolArgs {
            input: normalize_pathdiff_envelope(value),
            summary: AGENT_RUN_EDIT_DEFAULT_SUMMARY.to_owned(),
        }),
        ToolCallInput::Text(_) => Err("builtin.edit input 不能为空".to_owned()),
        ToolCallInput::Json(value) => {
            let input = extract_edit_input_from_json(value)?;
            let summary = optional_summary(value, AGENT_RUN_EDIT_DEFAULT_SUMMARY)?;
            Ok(AgentRunEditToolArgs {
                input: normalize_pathdiff_envelope(&input),
                summary,
            })
        }
    }
}

const EDIT_INPUT_ALIASES: &[&str] = &["input", "code_edit", "patch", "content", "diff", "text"];

fn extract_edit_input_from_json(value: &Value) -> Result<String, String> {
    for &key in EDIT_INPUT_ALIASES {
        if let Some(val) = value.get(key).and_then(Value::as_str)
            && !val.trim().is_empty()
        {
            return Ok(val.to_owned());
        }
    }
    Err("builtin.edit arguments 缺少 input（支持别名 code_edit / patch / content / diff）且内容不能为空".to_owned())
}

/// 判断 edit 输入是否为完整 Codex patch。
#[must_use]
pub fn agent_run_edit_input_is_apply_patch(input: &str) -> bool {
    let trimmed = input.trim_start_matches('\u{feff}').trim();
    trimmed.starts_with(APPLY_PATCH_BEGIN_MARKER) && trimmed.ends_with(APPLY_PATCH_END_MARKER)
}

/// 提取 edit 将写入的全部工具路径。
///
/// Hashline 的 MV 与 Codex patch 的 Move 都同时返回源路径和目标路径。
pub fn agent_run_edit_write_paths_from_provider_input(
    input: &ToolCallInput,
) -> Result<Vec<String>, String> {
    let input = agent_run_edit_tool_args_from_provider_input(input)?;
    if agent_run_edit_input_is_apply_patch(&input.input) {
        let hunks = axon_patch_model::parse_patch(&input.input)
            .map_err(|error| format!("builtin.edit patch 输入无效：{error}"))?;
        let mut paths = BTreeSet::new();
        for hunk in hunks {
            paths.insert(hunk.source_path().to_owned());
            paths.insert(hunk.path().to_owned());
        }
        return Ok(paths.into_iter().collect());
    }
    crate::hashline_write_paths(&input.input)
}

fn parse_arguments(arguments_json: &str, tool_name: &str) -> Result<Value, String> {
    serde_json::from_str(arguments_json).map_err(|_| format!("{tool_name} arguments JSON 无效"))
}

fn required_tool_path(value: &Value, field: &str, tool_name: &str) -> Result<PathBuf, String> {
    let raw = required_non_empty_string(value, field, tool_name)?;
    tool_path(&raw, field, tool_name)
}

fn tool_path(raw: &str, field: &str, tool_name: &str) -> Result<PathBuf, String> {
    let normalized = raw.replace('\\', "/");
    let path = PathBuf::from(normalized);
    if path.as_os_str().is_empty() {
        return Err(format!("{tool_name} {field} 不能为空"));
    }
    Ok(path)
}

fn required_string(value: &Value, field: &str, tool_name: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{tool_name} arguments 缺少 {field}"))
}

fn required_non_empty_string(
    value: &Value,
    field: &str,
    tool_name: &str,
) -> Result<String, String> {
    let raw = required_string(value, field, tool_name)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{tool_name} {field} 不能为空"));
    }
    Ok(trimmed.to_owned())
}

fn optional_summary(value: &Value, default_summary: &str) -> Result<String, String> {
    let Some(summary) = value.get("summary") else {
        return Ok(default_summary.to_owned());
    };
    let summary = summary
        .as_str()
        .filter(|summary| !summary.trim().is_empty())
        .ok_or_else(|| "builtin.edit summary 必须是非空字符串".to_owned())?;
    Ok(summary.to_owned())
}

fn optional_usize(value: &Value, field: &str, tool_name: &str) -> Result<Option<usize>, String> {
    value
        .get(field)
        .map(|item| {
            item.as_u64()
                .and_then(|raw| usize::try_from(raw).ok())
                .ok_or_else(|| format!("{tool_name} {field} 必须是非负整数"))
        })
        .transpose()
}

fn optional_u64(value: &Value, field: &str, tool_name: &str) -> Result<Option<u64>, String> {
    value
        .get(field)
        .map(|item| {
            item.as_u64()
                .ok_or_else(|| format!("{tool_name} {field} 必须是非负整数"))
        })
        .transpose()
}

fn optional_string(value: &Value, field: &str, tool_name: &str) -> Result<Option<String>, String> {
    value
        .get(field)
        .map(|item| {
            item.as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("{tool_name} {field} 必须是字符串"))
        })
        .transpose()
}

fn optional_shell_workdir(value: &Value, base_cwd: &Path) -> Result<PathBuf, String> {
    let Some(raw) = optional_string(value, "workdir", "builtin.shell_command")? else {
        return Ok(base_cwd.to_path_buf());
    };
    let path = raw.trim();
    if path.is_empty() {
        return Err("builtin.shell_command workdir 不能为空".to_owned());
    }
    let path = PathBuf::from(path.replace('\\', "/"));
    Ok(if is_absolute_shell_path(&path) {
        path
    } else {
        base_cwd.join(path)
    })
}

fn shell_workdir_from_raw(raw: &str, base_cwd: &Path) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw.replace('\\', "/"));
    Ok(if is_absolute_shell_path(&path) {
        path
    } else {
        base_cwd.join(path)
    })
}

fn is_absolute_shell_path(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    path.is_absolute()
        || (raw.as_bytes().get(1) == Some(&b':')
            && raw.as_bytes().get(2).is_some_and(|byte| *byte == b'/'))
        || raw.starts_with("//")
}

fn optional_bool(value: &Value, field: &str, tool_name: &str) -> Result<Option<bool>, String> {
    value
        .get(field)
        .map(|item| {
            item.as_bool()
                .ok_or_else(|| format!("{tool_name} {field} 必须是布尔值"))
        })
        .transpose()
}

fn optional_positive_usize(
    value: &Value,
    field: &str,
    tool_name: &str,
) -> Result<Option<usize>, String> {
    let parsed = optional_usize(value, field, tool_name)?;
    if parsed == Some(0) {
        return Err(format!("{tool_name} {field} 必须是正整数"));
    }
    Ok(parsed)
}

fn optional_string_array(
    value: &Value,
    field: &str,
    tool_name: &str,
) -> Result<Vec<String>, String> {
    let Some(raw) = value.get(field) else {
        return Ok(Vec::new());
    };
    if let Some(item) = raw.as_str() {
        if item.trim().is_empty() {
            return Err(format!(
                "{tool_name} {field} 只能是非空字符串或非空字符串数组"
            ));
        }
        return Ok(vec![item.to_owned()]);
    }
    let Some(items) = raw.as_array() else {
        return Err(format!("{tool_name} {field} 必须是非空字符串或字符串数组"));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("{tool_name} {field} 只能包含非空字符串"))
        })
        .collect()
}

fn optional_regex_mode(value: &Value) -> Result<RegexSearchMode, String> {
    let Some(raw) = value.get("regex_mode").and_then(Value::as_str) else {
        return Ok(RegexSearchMode::Literal);
    };
    match raw {
        "literal" => Ok(RegexSearchMode::Literal),
        "regex" => Ok(RegexSearchMode::Regex),
        "fixed_string" => Ok(RegexSearchMode::FixedString),
        _ => Err("builtin.search regex_mode 必须是 literal、regex 或 fixed_string".to_owned()),
    }
}

fn optional_glob_include(value: &Value) -> Result<GlobInclude, String> {
    let Some(raw) = value.get("include") else {
        return Ok(GlobInclude::All);
    };
    if let Some(single) = raw.as_str() {
        return parse_glob_include_values(std::slice::from_ref(&Value::String(single.to_owned())));
    }
    let Some(values) = raw.as_array() else {
        return Err("builtin.glob include 必须是 files、directories、all 或其非空数组".to_owned());
    };
    parse_glob_include_values(values)
}

fn parse_glob_include_values(values: &[Value]) -> Result<GlobInclude, String> {
    if values.is_empty() {
        return Err("builtin.glob include 数组不能为空".to_owned());
    }
    let mut has_files = false;
    let mut has_directories = false;
    for value in values {
        match value.as_str() {
            Some("files") => has_files = true,
            Some("directories") => has_directories = true,
            Some("all") => return Ok(GlobInclude::All),
            _ => return Err("builtin.glob include 必须只包含 files、directories 或 all".to_owned()),
        }
    }
    match (has_files, has_directories) {
        (true, true) => Ok(GlobInclude::All),
        (true, false) => Ok(GlobInclude::Files),
        (false, true) => Ok(GlobInclude::Directories),
        (false, false) => Err("builtin.glob include 数组不能为空".to_owned()),
    }
}

fn optional_filter_level(value: &Value) -> Result<Option<LspFilterLevel>, String> {
    value
        .get("filter_level")
        .map(|item| {
            item.as_str()
                .and_then(LspFilterLevel::parse)
                .ok_or_else(|| {
                    "builtin.read_file filter_level 必须是 none、minimal 或 aggressive".to_owned()
                })
        })
        .transpose()
}

fn shell_command_argument_error(error: AxonError) -> String {
    match error {
        AxonError::Tool(ToolError::InvalidArguments(message)) => message,
        AxonError::Tool(ToolError::ExecutionFailed(message)) => {
            format!("builtin.shell_command 执行准备失败：{message}")
        }
        AxonError::Tool(ToolError::Timeout { timeout_ms }) => {
            format!("builtin.shell_command 超时限制无效：{timeout_ms} 毫秒")
        }
        _ => "builtin.shell_command 参数无效，请传当前默认 shell 的裸脚本".to_owned(),
    }
}

#[cfg(test)]
#[path = "agent_run_tool_args_tests.rs"]
mod tests;
