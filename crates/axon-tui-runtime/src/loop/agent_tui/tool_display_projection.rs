//! 工具展示投影模型。

use axon_display::{ToolRowTarget, builtin_tool_display_label};
use serde_json::Value;

#[cfg(test)]
#[path = "tool_label_coverage_tests.rs"]
mod tool_label_coverage_tests;

use super::{
    run_output::AgentTuiToolKind, tool_output_projection::shared_executed_command_preview,
};

const LABEL_READ: &str = "读取";
const LABEL_SEARCH: &str = "检索";
const LABEL_EDIT: &str = "编辑";
const LABEL_RUN: &str = "执行";
const LABEL_MCP: &str = "MCP";
const LABEL_SKILL: &str = "技能";
const LABEL_GLOB: &str = "匹配";
const LABEL_BACKGROUND_TERMINAL: &str = "后台终端";
const LABEL_WAITING_BACKGROUND_TERMINAL: &str = "等待终端";
const LABEL_TERMINAL_EXECUTION: &str = "终端执行";
const LABEL_WAITED_BACKGROUND_TERMINAL: &str = "已等待终端";

const TARGET_MEMORY: &str = "记忆";
const TARGET_HISTORY: &str = "历史";
const TARGET_NOTES: &str = "便签";

const META_DISPLAY_TARGET: &str = "displayTarget";
const META_TARGET: &str = "target";
const META_DISPLAY_DETAIL: &str = "displayDetail";
const META_DETAIL: &str = "detail";
const META_MCP_SERVER: &str = "server";
const META_MCP_TOOL: &str = "tool";
const META_ARGUMENTS_PREVIEW: &str = "argumentsPreview";
const META_TERMINAL_INTERACTION: &str = "terminalInteraction";
const META_INTERACTION_KIND: &str = "kind";
const META_INTERACTION_STDIN: &str = "stdin";
const META_STATUS: &str = "status";
const INTERACTION_WAIT: &str = "wait";
const STATUS_RUNNING: &str = "running";
const STATUS_STARTED: &str = "started";
const STATUS_WAITING: &str = "waiting";

/// 工具调用在 transcript 中使用的统一展示投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDisplayProjection {
    /// 主标题短标签。
    pub label: String,
    /// 副标题目标，例如 Memory 或 gitnexus.query。
    pub target: Option<String>,
    /// provider/runtime 原始工具名。
    pub raw_name: String,
    /// 仅在详情中展示的调试信息。
    pub detail: Option<String>,
}

impl ToolDisplayProjection {
    /// 从工具调用参数生成执行中的展示投影。
    ///
    /// `intent` 是模型自述的调用目的：有它时折叠态只显示这一句人话，
    /// 命令原文留给展开态；没有时保持既有参数投影，绝不猜意图。
    #[must_use]
    pub fn from_tool_call(
        tool_name: &str,
        kind: AgentTuiToolKind,
        arguments_json: &str,
        intent: Option<&str>,
    ) -> Self {
        let raw_name = tool_name.trim();
        if kind == AgentTuiToolKind::Mcp || raw_name.starts_with("mcp__") {
            return Self::mcp(raw_name, None);
        }
        if is_write_stdin_tool(raw_name) {
            let label = if write_stdin_waits(arguments_json) {
                LABEL_WAITING_BACKGROUND_TERMINAL
            } else {
                LABEL_TERMINAL_EXECUTION
            };
            return Self {
                label: label.to_owned(),
                // 终端交互同样有模型自述目的：折叠态给这一句，会话 ID 与字符留给展开态。
                target: row_target_text(raw_name, arguments_json, intent),
                raw_name: raw_name.to_owned(),
                detail: None,
            };
        }
        let mut projection = Self::from_tool(raw_name, kind, None);
        match row_target(raw_name, arguments_json, intent) {
            Some(ToolRowTarget::Text(text)) => projection.target = Some(text),
            // 规则要求静默时不得回落到参数投影：参数本身对用户没有信息量。
            Some(ToolRowTarget::Silent) => projection.target = None,
            None => {}
        }
        projection
    }

    /// 从原始工具名、语义类型和 metadata 生成展示投影。
    #[must_use]
    pub fn from_tool(tool_name: &str, kind: AgentTuiToolKind, metadata: Option<&Value>) -> Self {
        let raw_name = tool_name.trim().to_owned();
        if kind == AgentTuiToolKind::Mcp || raw_name.starts_with("mcp__") {
            return Self::mcp(&raw_name, metadata);
        }
        if is_write_stdin_tool(&raw_name) {
            return Self::terminal_interaction(&raw_name, metadata);
        }
        let label = builtin_tool_display_label(&raw_name)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if raw_name.is_empty() {
                    kind.display_label().to_owned()
                } else {
                    raw_name.clone()
                }
            });
        let rule = row_target(&raw_name, &arguments_preview(metadata), None);
        let target = metadata_target(metadata).or_else(|| match rule {
            Some(ToolRowTarget::Text(text)) => Some(text),
            // 规则要求静默时不回落到类型默认目标。
            Some(ToolRowTarget::Silent) => None,
            None => kind_default_target(kind),
        });
        let detail =
            metadata_detail(metadata).or_else(|| raw_detail(&raw_name, &label, target.as_deref()));
        Self {
            label,
            target,
            raw_name,
            detail,
        }
    }

    fn mcp(raw_name: &str, metadata: Option<&Value>) -> Self {
        let target = metadata_mcp_target(metadata);
        let detail = metadata_detail(metadata)
            .or_else(|| raw_detail(raw_name, LABEL_MCP, target.as_deref()));
        Self {
            label: LABEL_MCP.to_owned(),
            target,
            raw_name: raw_name.to_owned(),
            detail,
        }
    }

    fn terminal_interaction(raw_name: &str, metadata: Option<&Value>) -> Self {
        let interaction = metadata.and_then(|value| value.get(META_TERMINAL_INTERACTION));
        let kind = interaction
            .and_then(|value| value.get(META_INTERACTION_KIND))
            .and_then(Value::as_str);
        let status = metadata
            .and_then(|value| value.get(META_STATUS))
            .and_then(Value::as_str);
        let label = match kind {
            Some(INTERACTION_WAIT) if terminal_status_is_active(status) => {
                LABEL_WAITING_BACKGROUND_TERMINAL
            }
            Some(INTERACTION_WAIT) => LABEL_WAITED_BACKGROUND_TERMINAL,
            Some(_) => LABEL_TERMINAL_EXECUTION,
            None => LABEL_BACKGROUND_TERMINAL,
        };
        // 折叠态先给模型自述目的，其次仍是执行过的命令预览。
        let arguments = arguments_preview(metadata);
        let target = row_target_text(raw_name, &arguments, None).or_else(|| {
            let preview = shared_executed_command_preview(&arguments);
            (!preview.is_empty()).then_some(preview)
        });
        let detail = interaction.and_then(|value| preview_field(value, META_INTERACTION_STDIN));
        Self {
            label: label.to_owned(),
            target,
            raw_name: raw_name.to_owned(),
            detail,
        }
    }
}

pub(crate) const fn tool_kind_display_label(kind: AgentTuiToolKind) -> &'static str {
    match kind {
        AgentTuiToolKind::Search => LABEL_SEARCH,
        AgentTuiToolKind::ReadFile => LABEL_READ,
        AgentTuiToolKind::Glob => LABEL_GLOB,
        AgentTuiToolKind::MemorySearch => "记忆检索",
        AgentTuiToolKind::MemoryWrite => "记忆写入",
        AgentTuiToolKind::History => TARGET_HISTORY,
        AgentTuiToolKind::Notes => TARGET_NOTES,
        AgentTuiToolKind::Mcp => LABEL_MCP,
        AgentTuiToolKind::InvokeSkill => LABEL_SKILL,
        AgentTuiToolKind::Shell => LABEL_RUN,
        AgentTuiToolKind::Edit => LABEL_EDIT,
        AgentTuiToolKind::Unsupported => "工具",
    }
}

fn kind_default_target(kind: AgentTuiToolKind) -> Option<String> {
    match kind {
        AgentTuiToolKind::MemorySearch => Some(TARGET_MEMORY.to_owned()),
        AgentTuiToolKind::History => Some(TARGET_HISTORY.to_owned()),
        AgentTuiToolKind::Notes => Some(TARGET_NOTES.to_owned()),
        _ => None,
    }
}

fn metadata_target(metadata: Option<&Value>) -> Option<String> {
    metadata.and_then(|value| {
        preview_field(value, META_DISPLAY_TARGET).or_else(|| preview_field(value, META_TARGET))
    })
}

fn metadata_detail(metadata: Option<&Value>) -> Option<String> {
    metadata.and_then(|value| {
        preview_field(value, META_DISPLAY_DETAIL).or_else(|| preview_field(value, META_DETAIL))
    })
}

fn metadata_mcp_target(metadata: Option<&Value>) -> Option<String> {
    let metadata = metadata?;
    let server = preview_field(metadata, META_MCP_SERVER)?;
    let tool = preview_field(metadata, META_MCP_TOOL);
    Some(match tool {
        Some(tool) => format!("{server}.{tool}"),
        None => server,
    })
}

fn raw_detail(raw_name: &str, label: &str, target: Option<&str>) -> Option<String> {
    if raw_name.is_empty() {
        return None;
    }
    if raw_name == label || target == Some(raw_name) {
        None
    } else {
        Some(raw_name.to_owned())
    }
}

fn is_write_stdin_tool(tool_name: &str) -> bool {
    matches!(tool_name, "builtin.write_stdin" | "write_stdin")
}

fn write_stdin_waits(arguments_json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(arguments_json) else {
        return false;
    };
    let ctrl_c = value
        .get("ctrl_c")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let has_stdin = value
        .get("chars")
        .and_then(Value::as_str)
        .is_some_and(|chars| !chars.is_empty());
    !ctrl_c && !has_stdin
}

fn preview_field(metadata: &Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// 结果元数据里回执的调用参数摘要；缺失时为空串。
fn arguments_preview(metadata: Option<&Value>) -> String {
    metadata
        .and_then(|value| preview_field(value, META_ARGUMENTS_PREVIEW))
        .unwrap_or_default()
}

/// 折叠态主文本的渠道无关规则：模型自述目的优先，其次工具专用主参数。
///
/// 自述目的先取显式值，再按声明该字段的工具从调用参数里取一次；
/// 未声明该字段的工具不会被参数里的同名内容覆盖。
fn row_target(raw_name: &str, arguments_json: &str, intent: Option<&str>) -> Option<ToolRowTarget> {
    let intent = intent
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| axon_display::tool_call_intent(raw_name, arguments_json));
    axon_display::tool_row_target(raw_name, arguments_json, intent.as_deref())
}

/// 规则给出的折叠态主文本；没有专用主事实（或规则要求静默）时为 `None`。
fn row_target_text(raw_name: &str, arguments_json: &str, intent: Option<&str>) -> Option<String> {
    match row_target(raw_name, arguments_json, intent) {
        Some(ToolRowTarget::Text(text)) => Some(text),
        Some(ToolRowTarget::Silent) | None => None,
    }
}

fn terminal_status_is_active(status: Option<&str>) -> bool {
    matches!(
        status,
        Some(STATUS_RUNNING | STATUS_STARTED | STATUS_WAITING)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_memory_search_projects_to_search_memory() {
        let projection = ToolDisplayProjection::from_tool(
            "builtin.memory_search",
            AgentTuiToolKind::MemorySearch,
            None,
        );

        assert_eq!(projection.label, "记忆检索");
        assert_eq!(projection.target.as_deref(), Some("记忆"));
        assert_eq!(projection.raw_name, "builtin.memory_search");
    }

    #[test]
    fn collapsed_targets_follow_the_shared_rule_table() {
        let cases = [
            (
                "builtin.memory_search",
                r#"{"limit":8,"query":"仓库文档重整"}"#,
                "记忆检索",
                "仓库文档重整",
            ),
            (
                "builtin.invoke_skill",
                r#"{"skill_name":"open-source-docs"}"#,
                "技能",
                "open-source-docs",
            ),
            (
                "builtin.delegate_to_fast",
                r#"{"run_in_background":true,"task":"审计 docs"}"#,
                "委托快速代理",
                "审计 docs",
            ),
            (
                "builtin.send_message",
                r#"{"message":"尽快返回结论"}"#,
                "发送消息",
                "尽快返回结论",
            ),
            (
                "job_output",
                r#"{"job_id":"1","yield_time_ms":0}"#,
                "作业输出",
                "等待 1 结果",
            ),
            (
                "builtin.new_context",
                r#"{"reason":"清理已完成，准备换窗"}"#,
                "新建上下文",
                "清理已完成，准备换窗",
            ),
            (
                "builtin.glob",
                r#"{"paths":"src","intent":"列出源码目录"}"#,
                "匹配",
                "列出源码目录",
            ),
        ];
        for (tool_name, arguments, label, target) in cases {
            let projection = ToolDisplayProjection::from_tool_call(
                tool_name,
                AgentTuiToolKind::from_tool_name(tool_name),
                arguments,
                None,
            );
            assert_eq!(projection.label, label, "{tool_name} 的折叠态标签");
            assert_eq!(
                projection.target.as_deref(),
                Some(target),
                "{tool_name} 的折叠态目标"
            );
        }
    }

    #[test]
    fn silent_rule_leaves_no_collapsed_target() {
        let projection = ToolDisplayProjection::from_tool_call(
            "builtin.get_context_remaining",
            AgentTuiToolKind::Unsupported,
            r#"{"__axon_tool_call_parse_error":"empty_arguments"}"#,
            None,
        );

        assert_eq!(projection.label, "查看剩余上下文");
        assert_eq!(projection.target, None, "参数没有信息量时不得拼接目标");
    }

    #[test]
    fn completed_tool_row_uses_the_same_rule_from_receipt_arguments() {
        let metadata = serde_json::json!({
            "argumentsPreview": r#"{"skill_name":"open-source-docs"}"#
        });
        let projection = ToolDisplayProjection::from_tool(
            "builtin.invoke_skill",
            AgentTuiToolKind::InvokeSkill,
            Some(&metadata),
        );

        assert_eq!(projection.label, "技能");
        assert_eq!(projection.target.as_deref(), Some("open-source-docs"));
    }

    #[test]
    fn mcp_tool_projects_structured_server_tool_metadata() {
        let metadata = serde_json::json!({
            "server": "gitnexus",
            "tool": "query"
        });
        let projection = ToolDisplayProjection::from_tool(
            "mcp__gitnexus__query",
            AgentTuiToolKind::Mcp,
            Some(&metadata),
        );

        assert_eq!(projection.label, "MCP");
        assert_eq!(projection.target.as_deref(), Some("gitnexus.query"));
        assert_eq!(projection.raw_name, "mcp__gitnexus__query");
    }

    #[test]
    fn write_stdin_projects_running_and_completed_terminal_semantics() {
        let waiting = ToolDisplayProjection::from_tool_call(
            "builtin.write_stdin",
            AgentTuiToolKind::Shell,
            r#"{"session_id":"session-1"}"#,
            None,
        );
        assert_eq!(waiting.label, "等待终端");

        let interacting = ToolDisplayProjection::from_tool_call(
            "builtin.write_stdin",
            AgentTuiToolKind::Shell,
            r#"{"session_id":"session-1","chars":"yes\n"}"#,
            None,
        );
        assert_eq!(interacting.label, "终端执行");

        let with_intent = ToolDisplayProjection::from_tool_call(
            "builtin.write_stdin",
            AgentTuiToolKind::Shell,
            r#"{"session_id":"session-1"}"#,
            Some(" 确认构建已结束 "),
        );
        assert_eq!(with_intent.label, "等待终端");
        assert_eq!(with_intent.target.as_deref(), Some("确认构建已结束"));

        let completed = ToolDisplayProjection::from_tool(
            "builtin.write_stdin",
            AgentTuiToolKind::Shell,
            Some(&serde_json::json!({
                "argumentsPreview": "cargo test --workspace",
                "terminalInteraction": {"kind": "wait", "stdin": ""}
            })),
        );
        assert_eq!(completed.label, "已等待终端");
        assert_eq!(completed.target.as_deref(), Some("cargo test --workspace"));
        assert_eq!(completed.detail, None);
    }

    #[test]
    fn shell_intent_becomes_the_collapsed_target_and_empty_intent_is_ignored() {
        let with_intent = ToolDisplayProjection::from_tool_call(
            "builtin.shell_command",
            AgentTuiToolKind::Shell,
            r#"{"command":"cargo fmt --all --check"}"#,
            Some(" 校验代码格式 "),
        );
        assert_eq!(with_intent.label, "执行");
        assert_eq!(with_intent.target.as_deref(), Some("校验代码格式"));

        let blank = ToolDisplayProjection::from_tool_call(
            "builtin.shell_command",
            AgentTuiToolKind::Shell,
            r#"{"command":"cargo fmt --all --check"}"#,
            Some("   "),
        );
        assert_eq!(blank.target, None, "空概要不得造出折叠态目标");
    }

    #[test]
    fn history_search_contents_projects_to_search_history() {
        let projection = ToolDisplayProjection::from_tool_call(
            "builtin.history_search_contents",
            AgentTuiToolKind::History,
            r#"{"query":"compile error"}"#,
            None,
        );
        assert_eq!(projection.label, "检索历史");
        assert_eq!(projection.target.as_deref(), Some("历史"));
        assert_eq!(projection.raw_name, "builtin.history_search_contents");
    }

    #[test]
    fn notes_read_file_projects_to_read_notes() {
        let projection = ToolDisplayProjection::from_tool_call(
            "builtin.notes_read_file",
            AgentTuiToolKind::Notes,
            r#"{"path":"checkpoint.md"}"#,
            None,
        );
        assert_eq!(projection.label, "读取便签");
        assert_eq!(projection.target.as_deref(), Some("便签"));
        assert_eq!(projection.raw_name, "builtin.notes_read_file");
    }
}
