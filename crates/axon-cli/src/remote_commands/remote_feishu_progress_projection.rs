//! 飞书进度卡的内容渲染（纯函数，无 I/O、无状态）。
//!
//! 投影正文必须满足两条硬约束：
//! 1. `MEDIA:` 协议行**整行隐藏**，且尾部可能是该协议行前缀的半截内容要**扣留**，
//!    否则用户在流式过程中会看到 `MEDIA:` 或半截协议行；
//! 2. 只渲染已确认稳定的内容，且始终受卡片预算约束。

use axon_agent_runtime::AgentReplyAttachmentRuntime;
use axon_display::builtin_tool_display_label;

/// `MEDIA:` 协议行前缀；渲染时整行隐藏。
const MEDIA_DIRECTIVE_PREFIX: &str = "MEDIA:";
/// 进度卡正文上限：留出卡片元数据预算，超出部分按尾部截断。
const PROGRESS_CARD_MAX_CHARS: usize = 8_000;
/// 工具卡摘要上限：只投影有界摘要，绝不投影原始工具输出。
const TOOL_SUMMARY_MAX_CHARS: usize = 400;
/// 折叠态目标上限：概要与主参数都收敛到一行。
const TOOL_TARGET_MAX_CHARS: usize = 120;
/// 主参数取值优先级；与 TUI/桌面端的折叠态目标同序。
const TOOL_TARGET_PRIMARY_KEYS: [&str; 8] = [
    "command", "path", "query", "url", "pattern", "target", "task", "skill",
];

/// 工具进度状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolProgressStatus {
    /// 已开始，尚未返回结果。
    Running,
    /// 已成功完成。
    Completed,
    /// 已失败或未完成。
    Failed,
}

/// 渲染进度卡正文：隐藏 `MEDIA:` 协议行，并扣留可能是协议行前缀的尾部。
///
/// 尾部只扣留「去掉前导空白后是 `MEDIA:` 前缀」的内容，其余正文照常实时投影。
#[must_use]
pub(crate) fn render_progress_body(buffer: &str) -> String {
    let holdback_at = holdback_start(buffer);
    let visible = AgentReplyAttachmentRuntime::visible_text(&buffer[..holdback_at]);
    let visible = visible.trim_end();
    if visible.chars().count() <= PROGRESS_CARD_MAX_CHARS {
        return visible.to_owned();
    }
    let mut truncated = visible
        .chars()
        .take(PROGRESS_CARD_MAX_CHARS)
        .collect::<String>();
    truncated.push('…');
    truncated
}

/// 渲染工具卡正文。
///
/// 只使用 wire 事件里已脱敏的 `summary` 字段，**绝不投影 `output_preview` 等原始输出**，
/// 并对摘要做有界截断。
#[must_use]
pub(crate) fn render_tool_body(
    tool_name: &str,
    summary: &str,
    status: ToolProgressStatus,
) -> String {
    let name = if tool_name.trim().is_empty() {
        TOOL_NAME_FALLBACK
    } else {
        tool_name.trim()
    };
    let marker = match status {
        ToolProgressStatus::Running => TOOL_RUNNING_MARKER,
        ToolProgressStatus::Completed => TOOL_COMPLETED_MARKER,
        ToolProgressStatus::Failed => TOOL_FAILED_MARKER,
    };
    let mut body = format!("{marker} **{}**", display_tool_name(name));
    let summary = bounded_summary(summary);
    if !summary.is_empty() {
        body.push('\n');
        body.push_str(&summary);
    }
    if matches!(status, ToolProgressStatus::Running) {
        body.push_str("\n\n_执行中…_");
    }
    body
}

/// 工具卡标题：内置工具用中文显示名，MCP 工具用统一缩写，未识别时保留原始名。
fn display_tool_name(tool_name: &str) -> String {
    if axon_display::mcp_tool_row_target(tool_name).is_some() {
        return LABEL_MCP.to_owned();
    }
    builtin_tool_display_label(tool_name).map_or_else(|| tool_name.to_owned(), str::to_owned)
}

/// 折叠态目标：模型自述目的 → 工具专用主参数 → 通用主参数 → 原始参数摘要。
///
/// 与桌面端、TUI 共用 `axon_display` 的规则表；规则要求静默时返回 `None`，
/// 表示这张卡不再追加任何目标行，调用方**不得**再回落到原始参数摘要。
#[must_use]
pub(crate) fn render_tool_target(
    tool_name: &str,
    arguments_summary: Option<&str>,
    intent: Option<&str>,
) -> Option<String> {
    let arguments = arguments_summary.map(str::trim).unwrap_or_default();
    match axon_display::tool_row_target(tool_name, arguments, intent) {
        Some(axon_display::ToolRowTarget::Text(text)) => return Some(bound_target(&text)),
        Some(axon_display::ToolRowTarget::Silent) => return None,
        None => {}
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) {
        for key in TOOL_TARGET_PRIMARY_KEYS {
            let Some(candidate) = value.get(key) else {
                continue;
            };
            let text = match candidate {
                serde_json::Value::String(text) => text.trim().to_owned(),
                serde_json::Value::Null => continue,
                other => other.to_string(),
            };
            if !text.is_empty() {
                return Some(bound_target(&text));
            }
        }
    }
    (!arguments.is_empty()).then(|| bound_target(arguments))
}

fn bound_target(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= TOOL_TARGET_MAX_CHARS {
        return normalized;
    }
    let mut truncated = normalized
        .chars()
        .take(TOOL_TARGET_MAX_CHARS)
        .collect::<String>();
    truncated.push('…');
    truncated
}

/// 工具结果未携带工具名时的兜底显示名。
const TOOL_NAME_FALLBACK: &str = "工具调用";
/// MCP 工具的统一显示名；服务名与工具名放在目标行。
const LABEL_MCP: &str = "MCP";
const TOOL_RUNNING_MARKER: &str = "🔧";
const TOOL_COMPLETED_MARKER: &str = "✅";
const TOOL_FAILED_MARKER: &str = "⚠️";

fn bounded_summary(summary: &str) -> String {
    let trimmed = summary.trim();
    if trimmed.chars().count() <= TOOL_SUMMARY_MAX_CHARS {
        return trimmed.to_owned();
    }
    let mut truncated = trimmed
        .chars()
        .take(TOOL_SUMMARY_MAX_CHARS)
        .collect::<String>();
    truncated.push('…');
    truncated
}

/// 返回可安全渲染的字节边界（其后的内容可能与 `MEDIA:` 协议行有关）。
fn holdback_start(buffer: &str) -> usize {
    let Some(line_start) = buffer.rfind('\n').map(|index| index + 1) else {
        return if is_media_prefix(buffer) {
            0
        } else {
            buffer.len()
        };
    };
    if line_start >= buffer.len() || !is_media_prefix(&buffer[line_start..]) {
        return buffer.len();
    }
    line_start
}

/// 判断该片段是否可能是 `MEDIA:` 协议行的前缀（去掉前导空白后）。
fn is_media_prefix(fragment: &str) -> bool {
    let trimmed = fragment.trim_start();
    !trimmed.is_empty() && MEDIA_DIRECTIVE_PREFIX.starts_with(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_body_hides_media_directive_lines() {
        assert_eq!(
            render_progress_body("说明\nMEDIA:C:\\tmp\\a.png\n结论"),
            "说明\n结论"
        );
    }

    #[test]
    fn progress_body_holds_back_possible_media_prefix() {
        for fragment in ["MED", "MEDIA", "MEDIA:"] {
            assert_eq!(
                render_progress_body(&format!("说明\n{fragment}")),
                "说明",
                "片段 {fragment} 应被扣留"
            );
        }
        assert_eq!(render_progress_body("说明\n媒体已生成"), "说明\n媒体已生成");
    }

    #[test]
    fn progress_body_truncates_beyond_card_budget() {
        let body = render_progress_body(&"x".repeat(PROGRESS_CARD_MAX_CHARS + 10));
        assert_eq!(body.chars().count(), PROGRESS_CARD_MAX_CHARS + 1);
        assert!(body.ends_with('…'));
    }

    #[test]
    fn tool_body_marks_lifecycle_and_bounds_summary() {
        assert_eq!(
            render_tool_body("shell_command", "运行构建", ToolProgressStatus::Running),
            "🔧 **执行**\n运行构建\n\n_执行中…_"
        );
        assert_eq!(
            render_tool_body("shell_command", "运行构建", ToolProgressStatus::Completed),
            "✅ **执行**\n运行构建"
        );
        assert_eq!(
            render_tool_body("shell_command", "运行构建", ToolProgressStatus::Failed),
            "⚠️ **执行**\n运行构建"
        );
        // 结果事件可能不带工具名；摘要必须被有界截断。
        let body = render_tool_body(
            "",
            &"y".repeat(TOOL_SUMMARY_MAX_CHARS + 5),
            ToolProgressStatus::Completed,
        );
        assert!(body.contains(TOOL_NAME_FALLBACK));
        assert_eq!(
            body.lines().nth(1).map(str::chars).map(Iterator::count),
            Some(TOOL_SUMMARY_MAX_CHARS + 1)
        );
    }

    #[test]
    fn tool_target_prefers_model_intent_over_command() {
        assert_eq!(
            render_tool_target(
                "shell_command",
                Some(r#"{"command":"cargo fmt --all --check"}"#),
                Some("校验代码格式")
            )
            .as_deref(),
            Some("校验代码格式")
        );
    }

    #[test]
    fn tool_target_falls_back_to_primary_argument() {
        assert_eq!(
            render_tool_target(
                "shell_command",
                Some(r#"{"command":"cargo test","yield_time_ms":1000}"#),
                None
            )
            .as_deref(),
            Some("cargo test"),
            "没有概要时给命令原文，而不是整包 JSON"
        );
        assert_eq!(
            render_tool_target("read_file", Some(r#"{"path":"crates/axon-cli"}"#), None).as_deref(),
            Some("crates/axon-cli")
        );
        assert_eq!(
            render_tool_target("read_file", Some("not json"), None).as_deref(),
            Some("not json")
        );
        assert_eq!(render_tool_target("shell_command", None, Some("   ")), None);
        assert_eq!(render_tool_target("thread_unknown", None, None), None);
    }

    #[test]
    fn tool_target_uses_shared_rules_per_tool() {
        let cases = [
            (
                "memory_search",
                r#"{"limit":8,"query":"仓库文档重整"}"#,
                "仓库文档重整",
            ),
            (
                "invoke_skill",
                r#"{"skill_name":"open-source-docs"}"#,
                "open-source-docs",
            ),
            ("delegate_to_fast", r#"{"task":"审计 docs"}"#, "审计 docs"),
            (
                "send_message",
                r#"{"message":"尽快返回结论"}"#,
                "尽快返回结论",
            ),
            (
                "job_output",
                r#"{"job_id":"1","yield_time_ms":0}"#,
                "等待 1 结果",
            ),
            (
                "new_context",
                r#"{"reason":"清理已完成，准备换窗"}"#,
                "清理已完成，准备换窗",
            ),
            (
                "mcp__codebase-memory-mcp__get_architecture",
                r#"{"project":"E-zbg-axon-code"}"#,
                "codebase-memory-mcp.get_architecture",
            ),
        ];
        for (tool_name, arguments, expected) in cases {
            assert_eq!(
                render_tool_target(tool_name, Some(arguments), None).as_deref(),
                Some(expected),
                "{tool_name} 的折叠态目标"
            );
        }
        assert_eq!(
            render_tool_target(
                "get_context_remaining",
                Some(r#"{"__axon_tool_call_parse_error":"empty_arguments"}"#),
                None
            ),
            None,
            "参数没有信息量时不得拼接任何目标"
        );
    }

    #[test]
    fn mcp_cards_use_shared_label_and_target() {
        let body = render_tool_body(
            "mcp__team_tools__query_db",
            "查询用户表",
            ToolProgressStatus::Running,
        );
        assert!(body.contains("**MCP**"));
        assert!(!body.contains("mcp__team_tools__query_db"));
    }

    #[test]
    fn tool_target_collapses_lines_and_bounds_length() {
        let long = format!(r#"{{"task":"审计 {}"}}"#, "x".repeat(400));
        let target = render_tool_target("delegate_to_fast", Some(&long), None).expect("任务原文");
        assert_eq!(target.chars().count(), TOOL_TARGET_MAX_CHARS + 1);
        assert!(target.ends_with('…'));
        assert_eq!(
            render_tool_target(
                "send_message",
                Some(r#"{"message":"先   看\n  日志"}"#),
                None
            )
            .as_deref(),
            Some("先 看 日志")
        );
    }
}
