//! 飞书进度卡的折叠态渲染（纯函数，无 I/O、无状态）。
//!
//! 卡片结构固定：header + **一个** markdown 元素。过程行在上、结论在下、终态页脚收尾。
//! 过程行不提供展开，因此每一行必须自足：
//!
//! ```text
//! {标记} **{类别}** · {目标} · {读数}
//! ```
//!
//! 三条硬约束：
//! 1. `MEDIA:` 协议行整行隐藏，尾部可能是该协议行前缀的半截内容要扣留；
//! 2. 过程行的目标与读数都有字符上限，保证在手机端仍然是一行；
//! 3. 卡片序列化后不得超过 `CARD_BUDGET_BYTES`，超出时先收过程行窗口、再收正文。

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use axon_agent_runtime::AgentReplyAttachmentRuntime;
use axon_display::builtin_tool_display_label;

use super::remote_feishu_progress_model::{
    Cell, NoticeRow, ProgressProjection, ReasoningRow, RowStatus, ToolRow,
};

/// `MEDIA:` 协议行前缀；渲染时整行隐藏。
const MEDIA_DIRECTIVE_PREFIX: &str = "MEDIA:";
/// 正文在单卡内的字符上限；超出即视为未完整投影，由 durable outbox 无损补投。
const PROGRESS_CARD_MAX_CHARS: usize = 8_000;
/// 折叠态目标上限：直立一行，给类别与读数留出位置。
const TOOL_TARGET_MAX_CHARS: usize = 80;
/// 行尾读数上限。
const TOOL_META_MAX_CHARS: usize = 24;
/// 过程行窗口：只留最近 5 行，更早的压成一行计数。
///
/// 卡片是一条向下生长的消息，结论始终在最底部；过程行留太多会把结论推远，
/// 也会让长回合的卡片在手机端失去可读性。
const PROCESS_WINDOW_ROWS: usize = 5;
/// 收过程行窗口时的单次步长。
const PROCESS_WINDOW_STEP: usize = 5;
/// 卡片序列化安全预算，低于飞书 30KB 上限。
const CARD_BUDGET_BYTES: usize = 28_000;

/// 折叠行的段间分隔。
const ROW_SEPARATOR: &str = " · ";
/// 运行中的标记。
const MARK_RUNNING: &str = "◌";
/// 已完成的标记。
const MARK_DONE: &str = "✓";
/// 已失败的标记。
const MARK_FAILED: &str = "✕";
/// 提示行的标记。
const MARK_NOTICE: &str = "!";
/// 思考行的类别名。
const LABEL_REASONING: &str = "思考";
/// 工具结果未携带工具名时的类别名。
const LABEL_TOOL_FALLBACK: &str = "工具";
/// MCP 工具的类别名；服务名与工具名放在目标行。
const LABEL_MCP: &str = "MCP";
/// 主参数取值优先级；与 TUI、桌面端的折叠态目标同序。
const TOOL_TARGET_PRIMARY_KEYS: [&str; 8] = [
    "command", "path", "query", "url", "pattern", "target", "task", "skill",
];

/// 卡片所处的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardStatus {
    /// 回合仍在执行。
    Running,
    /// 回合正常结束。
    Completed,
    /// 回合失败结束。
    Failed,
}

impl CardStatus {
    /// header 标题里的状态词。
    fn word(self) -> &'static str {
        match self {
            Self::Running => "执行中",
            Self::Completed => "已完成",
            Self::Failed => "已失败",
        }
    }
}

/// 已渲染的卡片。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedCard {
    /// 序列化后的卡片 JSON，直接作为飞书 `content`。
    pub(crate) content_json: String,
    /// 卡片指纹：内容不变就不必再发 PATCH。
    pub(crate) fingerprint: u64,
    /// 本卡应当承载的正文（已剥离协议行、未截断）。
    pub(crate) body_text: String,
    /// 本卡是否完整承载了正文；被预算截断时为 `false`。
    pub(crate) body_complete: bool,
}

/// 渲染后的正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedBody {
    /// 可见正文。
    pub(crate) text: String,
    /// 是否被字符上限截断。
    pub(crate) truncated: bool,
}

/// 渲染整张进度卡。
///
/// `elapsed_ms` 只在终态页脚出现：运行期不渲染耗时，避免每秒一次的无效 PATCH。
#[must_use]
pub(crate) fn render_card(
    projection: &ProgressProjection,
    status: CardStatus,
    elapsed_ms: u64,
) -> RenderedCard {
    let raw_body = projection.body_text();
    let body = render_progress_body(&raw_body);
    let rows = row_lines(projection);
    let footer = match status {
        CardStatus::Running => None,
        CardStatus::Completed | CardStatus::Failed => {
            Some(footer_line(projection, status, elapsed_ms))
        }
    };
    let title = format!("Axon{ROW_SEPARATOR}{}", status.word());

    let mut body_text = body.text;
    let mut body_complete = !body.truncated;
    let mut window = PROCESS_WINDOW_ROWS;
    let mut content_json = card_json(
        &title,
        &assemble(&body_text, &rows, footer.as_deref(), window),
    )
    .to_string();

    // 先收过程行窗口：正文是这条消息的主体，最后才让位。
    while content_json.len() > CARD_BUDGET_BYTES && window > 0 {
        window = window.saturating_sub(PROCESS_WINDOW_STEP);
        content_json = card_json(
            &title,
            &assemble(&body_text, &rows, footer.as_deref(), window),
        )
        .to_string();
    }
    // 正文仍然放不下时按四分之一递减；截断即代表未完整投影，由 outbox 补投。
    while content_json.len() > CARD_BUDGET_BYTES && !body_text.is_empty() {
        let chars = body_text.chars().count();
        let keep = chars.saturating_sub(chars / 4 + 1);
        body_text = body_text.chars().take(keep).collect();
        body_complete = false;
        content_json = card_json(
            &title,
            &assemble(&body_text, &rows, footer.as_deref(), window),
        )
        .to_string();
    }

    RenderedCard {
        fingerprint: fingerprint_of(&content_json),
        content_json,
        body_text: render_progress_body(&raw_body).text,
        body_complete,
    }
}

/// 渲染正文：隐藏 `MEDIA:` 协议行，并扣留可能是协议行前缀的尾部。
#[must_use]
pub(crate) fn render_progress_body(buffer: &str) -> RenderedBody {
    let holdback_at = holdback_start(buffer);
    let visible = AgentReplyAttachmentRuntime::visible_text(&buffer[..holdback_at]);
    let visible = visible.trim_end();
    if visible.chars().count() <= PROGRESS_CARD_MAX_CHARS {
        return RenderedBody {
            text: visible.to_owned(),
            truncated: false,
        };
    }
    let mut truncated = visible
        .chars()
        .take(PROGRESS_CARD_MAX_CHARS)
        .collect::<String>();
    truncated.push('…');
    RenderedBody {
        text: truncated,
        truncated: true,
    }
}

/// 工具类别显示名：内置工具用中文显示名，MCP 工具用统一缩写，未识别时保留原始名。
#[must_use]
pub(crate) fn render_tool_label(tool_name: &str) -> String {
    let name = tool_name.trim();
    if name.is_empty() {
        return LABEL_TOOL_FALLBACK.to_owned();
    }
    if axon_display::mcp_tool_row_target(name).is_some() {
        return LABEL_MCP.to_owned();
    }
    builtin_tool_display_label(name).map_or_else(|| name.to_owned(), str::to_owned)
}

/// 折叠态目标：模型自述目的 → 工具专用主参数 → 通用主参数 → 原始参数摘要。
///
/// 与桌面端、TUI 共用 `axon_display` 的规则表；规则要求静默时返回 `None`，
/// 表示这一行不再追加任何目标，调用方**不得**再回落到原始参数摘要。
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

/// 行尾读数：`exit` 非零、失败、写入、耗时，按信息量从高到低取第一个。
#[must_use]
pub(crate) fn bound_tool_meta(value: &str) -> String {
    bound(value, TOOL_META_MAX_CHARS)
}

fn row_lines(projection: &ProgressProjection) -> Vec<String> {
    projection
        .cells()
        .iter()
        .filter_map(|cell| match cell {
            Cell::Body { .. } => None,
            Cell::Tool(row) => Some(tool_row_line(row)),
            Cell::Reasoning(row) => Some(reasoning_row_line(row)),
            Cell::Notice(row) => Some(notice_row_line(row)),
        })
        .collect()
}

fn tool_row_line(row: &ToolRow) -> String {
    let mut line = format!("{} **{}**", status_mark(row.status), row.label);
    if let Some(target) = row
        .target
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        line.push_str(ROW_SEPARATOR);
        line.push_str(&bound(target, TOOL_TARGET_MAX_CHARS));
    }
    if let Some(meta) = row
        .meta
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        line.push_str(ROW_SEPARATOR);
        line.push_str(&bound(meta, TOOL_META_MAX_CHARS));
    }
    line
}

fn reasoning_row_line(row: &ReasoningRow) -> String {
    match row.duration_ms {
        Some(duration_ms) => format!(
            "{MARK_DONE} **{LABEL_REASONING}**{ROW_SEPARATOR}{}",
            crate::display_units::format_duration_millis_compact(duration_ms)
        ),
        None => format!("{MARK_RUNNING} **{LABEL_REASONING}**"),
    }
}

fn notice_row_line(row: &NoticeRow) -> String {
    format!("{MARK_NOTICE} {}", row.text.trim())
}

fn status_mark(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Running => MARK_RUNNING,
        RowStatus::Completed => MARK_DONE,
        RowStatus::Failed => MARK_FAILED,
    }
}

fn footer_line(projection: &ProgressProjection, status: CardStatus, elapsed_ms: u64) -> String {
    let mut parts = vec![status.word().to_owned()];
    if elapsed_ms > 0 {
        parts.push(crate::display_units::format_duration_millis_compact(
            elapsed_ms,
        ));
    }
    if let Some(route) = projection.route().map(str::trim).filter(|v| !v.is_empty()) {
        parts.push(route.to_owned());
    }
    if let Some(tokens) = projection.context_tokens().filter(|tokens| *tokens > 0) {
        parts.push(format!("上下文 {}", compact_tokens(tokens)));
    }
    parts.join(ROW_SEPARATOR)
}

fn compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// 拼装卡片正文：过程行在上、结论在下、页脚收尾，块之间各留一个空行。
///
/// 顺序不能反过来：卡片是一条持续向下生长的消息，工具调用越多，正文越应留在
/// 最下面，否则读者每来一个工具行都要向上回滚才能看到结论。
fn assemble(body: &str, rows: &[String], footer: Option<&str>, window: usize) -> String {
    let mut out = String::new();
    if !rows.is_empty() && window > 0 {
        let start = rows.len().saturating_sub(window);
        if start > 0 {
            out.push_str(&format!("… {start} 个动作已省略\n"));
        }
        out.push_str(&rows[start..].join("\n"));
    }
    if !body.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(body);
    }
    if let Some(footer) = footer {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(footer);
    }
    out.trim().to_owned()
}

fn card_json(title: &str, markdown: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "header": {
            "title": { "tag": "plain_text", "content": title },
        },
        "body": {
            "elements": [{
                "tag": "markdown",
                "content": markdown,
            }],
        },
    })
}

fn fingerprint_of(content_json: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content_json.hash(&mut hasher);
    hasher.finish()
}

/// 单行化并压到 `max` 字符；超出时用省略号收尾。
fn bound(value: &str, max: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max {
        return normalized;
    }
    let mut truncated = normalized.chars().take(max).collect::<String>();
    truncated.push('…');
    truncated
}

fn bound_target(value: &str) -> String {
    bound(value, TOOL_TARGET_MAX_CHARS)
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
#[path = "remote_feishu_progress_projection_tests.rs"]
mod tests;
