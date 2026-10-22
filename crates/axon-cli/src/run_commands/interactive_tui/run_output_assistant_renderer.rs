use axon_protocol::wire::{WireEventEnvelope, WireEventKind};
use axon_tui_runtime::AgentTuiMessagePart;

use super::run_output_generation_renderer::RetryGenerationProjection;

const GENERATED_SUMMARY_FOOTER_PREFIXES: &[&str] = &[
    "任务完成：status=",
    "用量 Token",
    "summary status=",
    "usage token",
];
const GENERATED_TEXT_SUMMARY_PREFIXES: &[&str] = &[
    "会话 ",
    "会话:",
    "模型 ",
    "模型:",
    "工具 ",
    "工具:",
    "工具摘要",
    "工具摘要:",
    "工具统计",
    "工具统计:",
    "自修复 ",
    "自修复:",
    "重试 ",
    "重试:",
    "下一步 ",
    "建议下一步",
    "后续建议",
    "已完成 ",
    "已修改文件 ",
    "apply 摘要 ",
    "上次失败点 ",
    "失败点 ",
    "恢复上下文 ",
    "session:",
    "model:",
    "tools:",
    "tool summary:",
    "tool stats:",
    "next step:",
    "suggested next step",
    "last failure:",
    "resume context:",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeneratedAssistantLineKind {
    Footer,
    TextSummary,
    VerifySummary,
    ToolSummary,
    UsageSummary,
    CompletionSummary,
}

pub(super) fn project_assistant_content(content: &str) -> Option<String> {
    let lines = content.lines().collect::<Vec<_>>();
    let footer_start = lines
        .iter()
        .position(|line| is_generated_summary_footer(line))
        .unwrap_or(lines.len());
    let projected = lines[..footer_start]
        .iter()
        .copied()
        .filter(|line| !is_generated_summary_line(line))
        .collect::<Vec<_>>()
        .join("\n");
    let projected = projected.trim();
    (!projected.is_empty()).then(|| projected.to_owned())
}

pub(super) fn wire_assistant_messages(events: &[WireEventEnvelope]) -> Vec<AgentTuiMessagePart> {
    let mut messages = Vec::<ProjectedAssistantMessage>::new();
    let mut generation = RetryGenerationProjection::default();
    for event in events {
        if let Some(failed_part_ids) = generation.failed_parts_for_event(event) {
            messages.retain(|message| !failed_part_ids.contains(&message.part_id));
            continue;
        }
        let WireEventKind::AssistantMessage(message) = &event.kind else {
            continue;
        };
        if message.delta {
            generation.observe_part(&message.part_id);
        }
        let entry = upsert_projected_message(&mut messages, &message.part_id);
        if message.delta {
            entry.delta_content.push_str(&message.content);
        } else {
            entry.final_content = Some(message.content.clone());
        }
    }
    messages
        .into_iter()
        .filter_map(ProjectedAssistantMessage::into_part)
        .collect()
}

fn upsert_projected_message<'a>(
    messages: &'a mut Vec<ProjectedAssistantMessage>,
    part_id: &str,
) -> &'a mut ProjectedAssistantMessage {
    if let Some(index) = messages
        .iter()
        .position(|message| message.part_id == part_id)
    {
        return &mut messages[index];
    }
    let index = messages.len();
    messages.push(ProjectedAssistantMessage {
        part_id: part_id.to_owned(),
        delta_content: String::new(),
        final_content: None,
    });
    &mut messages[index]
}

struct ProjectedAssistantMessage {
    part_id: String,
    delta_content: String,
    final_content: Option<String>,
}

impl ProjectedAssistantMessage {
    fn into_part(self) -> Option<AgentTuiMessagePart> {
        let content = self
            .final_content
            .as_deref()
            .unwrap_or(self.delta_content.as_str());
        project_assistant_content(content)
            .map(|content| AgentTuiMessagePart::assistant(self.part_id, content))
    }
}

fn is_generated_summary_footer(line: &str) -> bool {
    matches!(
        classify_generated_assistant_line(line),
        Some(
            GeneratedAssistantLineKind::Footer
                | GeneratedAssistantLineKind::CompletionSummary
                | GeneratedAssistantLineKind::UsageSummary
        )
    )
}

fn is_generated_summary_line(line: &str) -> bool {
    classify_generated_assistant_line(line).is_some()
}

fn classify_generated_assistant_line(line: &str) -> Option<GeneratedAssistantLineKind> {
    let line = normalized_summary_line(line);
    let lower = line.to_ascii_lowercase();
    if GENERATED_SUMMARY_FOOTER_PREFIXES
        .iter()
        .any(|prefix| line.starts_with(prefix))
    {
        return Some(GeneratedAssistantLineKind::Footer);
    }
    if GENERATED_TEXT_SUMMARY_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return Some(GeneratedAssistantLineKind::TextSummary);
    }
    if is_generated_verify_summary_line(line) {
        return Some(GeneratedAssistantLineKind::VerifySummary);
    }
    if is_generated_tool_summary_line(line) {
        return Some(GeneratedAssistantLineKind::ToolSummary);
    }
    if is_generated_usage_summary_line(line) {
        return Some(GeneratedAssistantLineKind::UsageSummary);
    }
    if is_generated_completion_summary_line(line) {
        return Some(GeneratedAssistantLineKind::CompletionSummary);
    }
    None
}

fn is_generated_verify_summary_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    line.starts_with("验证停止 ")
        || line.starts_with("验证停止：")
        || line.starts_with("verify 停止")
        || lower.starts_with("verify stopped")
        || lower.starts_with("verify failed")
        || lower.starts_with("verify passed")
        || line.starts_with("验证 轮次 ")
        || (line.starts_with("验证 ") && line.contains(" · "))
}

fn is_generated_tool_summary_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("工具")
        && (line.contains("tools=")
            || line.contains("failed=")
            || line.contains("blocked=")
            || line.contains("latest=")
            || line.contains(" · "))
        || (lower.starts_with("tool")
            && (lower.contains("tools=")
                || lower.contains("failed=")
                || lower.contains("blocked=")
                || lower.contains("latest=")
                || lower.contains("completed=")))
        || (contains_machine_tool_field(&lower)
            && (lower.contains("tool") || line.contains("工具")))
}

fn is_generated_completion_summary_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    (line.contains("任务完成") || lower.starts_with("summary") || lower.starts_with("completed"))
        && lower.contains("status=")
        && (lower.contains("tools=") || lower.contains("tokens=") || lower.contains("verify="))
}

fn is_generated_usage_summary_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    (line.contains("Token") || lower.starts_with("usage") || lower.starts_with("token usage"))
        && lower.contains("total=")
        && (lower.contains("input=") || lower.contains("output="))
}

fn contains_machine_tool_field(lower: &str) -> bool {
    lower.contains("tools=")
        || lower.contains("failed=")
        || lower.contains("blocked=")
        || lower.contains("latest=")
        || lower.contains("completed=")
}

fn normalized_summary_line(line: &str) -> &str {
    line.trim_start_matches(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '-' | '*' | '•' | '▸' | '◌' | '✓' | '✔' | '✗' | '✘' | '✅' | '❌' | ':'
            )
    })
    .trim_start()
}
