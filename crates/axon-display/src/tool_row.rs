//! 工具行折叠态的渠道无关契约。
//!
//! 桌面端、TUI 与飞书渠道共用同一份规则表（`tool_row_rules.json`）：这些工具的参数里
//! 只有一项对用户有意义，其余的只是分页、预算一类执行细节。收起时只显示那一项，
//! 原始参数留给展开态。取值顺序固定：模型自述的 `intent` → MCP 服务名与工具名 →
//! 工具专用主参数；没有专用规则的工具不在这里猜内容，由各渠道按既有投影回落。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

/// 模型自述目的的字段名；所有声明该字段的内置工具共用同一个名字。
pub const TOOL_INTENT_FIELD: &str = "intent";
/// 展示用目的概要的最大字符数；只裁剪展示长度，不改写内容。
pub const TOOL_INTENT_MAX_CHARS: usize = 120;
/// `intent` 字段的统一说明：写目标而不是复述参数。
pub const TOOL_INTENT_FIELD_DESCRIPTION: &str = "必填：一句话说明这次调用要达成什么目的，例如「确认依赖版本」「定位未覆盖的模块」。写目标而不是复述参数，不加句号，不超过 40 字。";
/// 折叠态目标文本的展示上限；过长的任务与消息在收起时只留开头。
pub const TOOL_ROW_TARGET_MAX_CHARS: usize = 200;

const TOOL_ROW_RULES_JSON: &str = include_str!("tool_row_rules.json");
const BUILTIN_NAMESPACE_PREFIX: &str = "builtin.";
const MCP_TOOL_PREFIX: &str = "mcp__";
const MCP_TOOL_DELIMITER: &str = "__";
const MCP_TARGET_SEPARATOR: char = '.';
const TEMPLATE_PLACEHOLDER: &str = "{value}";
const TRUNCATION_SUFFIX: char = '…';

static TOOL_ROW_RULES: OnceLock<ToolRowRules> = OnceLock::new();

/// 折叠态主文本的取值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRowTarget {
    /// 收起时只显示这段文本。
    Text(String),
    /// 收起时不追加任何内容：参数本身对用户没有信息量。
    Silent,
}

#[derive(Debug, Deserialize)]
struct ToolRowRules {
    #[serde(default)]
    intent_tools: Vec<String>,
    #[serde(default)]
    targets: BTreeMap<String, ToolRowTargetRule>,
}

#[derive(Debug, Deserialize)]
struct ToolRowTargetRule {
    #[serde(default)]
    fields: Vec<String>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    silent: bool,
}

fn rules() -> &'static ToolRowRules {
    TOOL_ROW_RULES.get_or_init(|| {
        serde_json::from_str(TOOL_ROW_RULES_JSON).expect("工具行规则表必须是有效 JSON")
    })
}

/// 去掉命名空间后的工具裸名：`builtin.glob` 与 `glob` 同形。
#[must_use]
pub fn bare_tool_name(tool_name: &str) -> &str {
    let trimmed = tool_name.trim();
    trimmed
        .strip_prefix(BUILTIN_NAMESPACE_PREFIX)
        .unwrap_or(trimmed)
}

/// 声明 `intent` 字段的内置工具裸名。
#[must_use]
pub fn tool_intent_tool_names() -> &'static [String] {
    &rules().intent_tools
}

/// 该工具是否声明了 `intent` 字段。
#[must_use]
pub fn tool_declares_intent(tool_name: &str) -> bool {
    let name = bare_tool_name(tool_name);
    rules().intent_tools.iter().any(|declared| declared == name)
}

/// 从工具调用参数里取出模型自述的目的概要。
///
/// 未声明该字段的工具一律返回 `None`：MCP 工具与第三方 schema 里的同名参数
/// 不属于本契约，界面按各自规则回落，不被这里误覆盖。
#[must_use]
pub fn tool_call_intent(tool_name: &str, arguments_json: &str) -> Option<String> {
    if !tool_declares_intent(tool_name) {
        return None;
    }
    let value: Value = serde_json::from_str(arguments_json).ok()?;
    intent_from_value(&value)
}

/// MCP 工具名的服务名与工具名，形态为 `mcp__server__tool`。
#[must_use]
pub fn mcp_tool_row_target(tool_name: &str) -> Option<String> {
    let body = tool_name.trim().strip_prefix(MCP_TOOL_PREFIX)?;
    let (server, tool) = body.split_once(MCP_TOOL_DELIMITER)?;
    let server = server.trim();
    let tool = tool.trim();
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some(format!("{server}{MCP_TARGET_SEPARATOR}{tool}"))
}

/// 折叠态主文本：模型自述目的 → MCP 服务名与工具名 → 工具专用主参数。
///
/// 返回 `None` 表示这一行没有专用主事实，由调用方按既有投影回落；
/// 返回 `ToolRowTarget::Silent` 表示收起时**不得**追加任何内容。
#[must_use]
pub fn tool_row_target(
    tool_name: &str,
    arguments_json: &str,
    intent: Option<&str>,
) -> Option<ToolRowTarget> {
    if let Some(intent) = intent.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(ToolRowTarget::Text(bound_target(intent)));
    }
    if let Some(mcp) = mcp_tool_row_target(tool_name) {
        return Some(ToolRowTarget::Text(mcp));
    }
    let rule = rules().targets.get(bare_tool_name(tool_name))?;
    if rule.silent {
        return Some(ToolRowTarget::Silent);
    }
    let value = argument_value(arguments_json, &rule.fields)?;
    Some(ToolRowTarget::Text(match rule.template.as_deref() {
        Some(template) => bound_target(&template.replace(TEMPLATE_PLACEHOLDER, &value)),
        None => bound_target(&value),
    }))
}

/// 折叠态主事实是否已经落在标题行上。
///
/// 自述目的、MCP 名与专用规则给出的主文本都由标题行承载，参数行再说一遍只是噪声；
/// 调用方用它决定是否省略折叠态的原始参数摘要。
#[must_use]
pub fn tool_row_title_carries_primary(tool_name: &str, arguments_json: &str) -> bool {
    if mcp_tool_row_target(tool_name).is_some() {
        return true;
    }
    let name = bare_tool_name(tool_name);
    if rules().targets.contains_key(name) {
        return true;
    }
    tool_declares_intent(name)
        && serde_json::from_str::<Value>(arguments_json)
            .ok()
            .is_some_and(|value| intent_from_value(&value).is_some())
}

/// 从已解析的参数对象里取出并裁剪目的概要；空值与缺失同义。
fn intent_from_value(value: &Value) -> Option<String> {
    value
        .get(TOOL_INTENT_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|intent| !intent.is_empty())
        .map(|intent| intent.chars().take(TOOL_INTENT_MAX_CHARS).collect())
}

/// 按规则逐字段取第一个非空值，并折成单行。
fn argument_value(arguments_json: &str, fields: &[String]) -> Option<String> {
    let value: Value = serde_json::from_str(arguments_json).ok()?;
    let object = value.as_object()?;
    for field in fields {
        let Some(candidate) = object.get(field) else {
            continue;
        };
        let text = match candidate {
            Value::String(text) => text.trim().to_owned(),
            Value::Number(number) => number.to_string(),
            _ => continue,
        };
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !normalized.is_empty() {
            return Some(normalized);
        }
    }
    None
}

/// 折叠成单行并裁剪到展示上限；裁剪只影响展示，不改写内容。
fn bound_target(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= TOOL_ROW_TARGET_MAX_CHARS {
        return normalized;
    }
    let mut truncated = normalized
        .chars()
        .take(TOOL_ROW_TARGET_MAX_CHARS)
        .collect::<String>();
    truncated.push(TRUNCATION_SUFFIX);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_tools_expose_intent_and_others_stay_none() {
        assert_eq!(
            tool_call_intent("builtin.glob", r#"{"paths":"src","intent":" 定位模块 "}"#).as_deref(),
            Some("定位模块")
        );
        assert_eq!(
            tool_call_intent("glob", r#"{"intent":"定位模块"}"#).as_deref(),
            Some("定位模块")
        );
        assert_eq!(
            tool_call_intent("mcp__team__search", r#"{"intent":"第三方参数"}"#),
            None,
            "未声明该字段的工具不得被同名参数覆盖"
        );
        assert_eq!(
            tool_call_intent("builtin.read_file", r#"{"path":"a"}"#),
            None
        );
        assert!(tool_declares_intent("builtin.notes_write_file"));
        assert!(!tool_declares_intent("builtin.read_file"));
    }

    #[test]
    fn blank_missing_or_broken_intent_is_none() {
        assert_eq!(
            tool_call_intent("builtin.glob", r#"{"intent":"   "}"#),
            None
        );
        assert_eq!(tool_call_intent("builtin.glob", r#"{"paths":"src"}"#), None);
        assert_eq!(tool_call_intent("builtin.glob", "not json"), None);
        assert_eq!(
            tool_call_intent(
                "builtin.search",
                &format!(r#"{{"intent":"{}"}}"#, "目".repeat(200))
            )
            .map(|intent| intent.chars().count()),
            Some(TOOL_INTENT_MAX_CHARS)
        );
    }

    #[test]
    fn tool_row_target_prefers_intent_then_mcp_then_rule() {
        assert_eq!(
            tool_row_target(
                "builtin.memory_search",
                r#"{"limit":8,"query":"仓库文档重整"}"#,
                Some("回忆既有决策")
            ),
            Some(ToolRowTarget::Text("回忆既有决策".to_owned()))
        );
        assert_eq!(
            tool_row_target(
                "builtin.memory_search",
                r#"{"limit":8,"query":"仓库文档重整"}"#,
                None
            ),
            Some(ToolRowTarget::Text("仓库文档重整".to_owned()))
        );
        assert_eq!(
            tool_row_target("mcp__codebase-memory-mcp__get_architecture", "{}", None),
            Some(ToolRowTarget::Text(
                "codebase-memory-mcp.get_architecture".to_owned()
            ))
        );
    }

    #[test]
    fn tool_row_target_covers_each_declared_rule() {
        let cases = [
            (
                "invoke_skill",
                r#"{"skill_name":"open-source-docs"}"#,
                "open-source-docs",
            ),
            ("delegate_to_fast", r#"{"task":"审计 docs"}"#, "审计 docs"),
            ("spawn_agent", r#"{"task":"审计 docs"}"#, "审计 docs"),
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
        ];
        for (tool_name, arguments, expected) in cases {
            assert_eq!(
                tool_row_target(tool_name, arguments, None),
                Some(ToolRowTarget::Text(expected.to_owned())),
                "{tool_name} 折叠态主文本"
            );
        }
        assert_eq!(
            tool_row_target("new_context", "{}", None),
            None,
            "reason 缺省时不得凭空空造，交由调用方回落"
        );
        assert_eq!(
            tool_row_target("get_context_remaining", r#"{"empty":true}"#, None),
            Some(ToolRowTarget::Silent)
        );
    }

    #[test]
    fn tool_row_target_leaves_unknown_tools_to_the_caller() {
        assert_eq!(
            tool_row_target("read_file", r#"{"path":"a.rs"}"#, None),
            None
        );
        assert_eq!(
            tool_row_target("send_message", r#"{"target":"/root/a"}"#, None),
            None,
            "规则字段缺失时不得凭空空造"
        );
        assert_eq!(tool_row_target("send_message", "not json", None), None);
    }

    #[test]
    fn tool_row_title_carries_primary_only_when_a_rule_applies() {
        assert!(tool_row_title_carries_primary(
            "builtin.glob",
            r#"{"paths":"src","intent":"列出源码"}"#
        ));
        assert!(!tool_row_title_carries_primary(
            "builtin.glob",
            r#"{"paths":"src"}"#
        ));
        assert!(tool_row_title_carries_primary(
            "builtin.memory_search",
            r#"{"query":"a"}"#
        ));
        assert!(tool_row_title_carries_primary(
            "get_context_remaining",
            "{}"
        ));
        assert!(tool_row_title_carries_primary("mcp__team__query", "{}"));
        assert!(!tool_row_title_carries_primary(
            "builtin.read_file",
            r#"{"path":"a"}"#
        ));
        assert!(!tool_row_title_carries_primary("mcp__team", "{}"));
    }

    #[test]
    fn row_target_collapses_lines_and_bounds_length() {
        let long = "任".repeat(TOOL_ROW_TARGET_MAX_CHARS + 20);
        assert_eq!(
            tool_row_target("send_message", &format!(r#"{{"message":"{long}"}}"#), None),
            Some(ToolRowTarget::Text(format!(
                "{}{TRUNCATION_SUFFIX}",
                "任".repeat(TOOL_ROW_TARGET_MAX_CHARS)
            )))
        );
        assert_eq!(
            tool_row_target("send_message", r#"{"message":"先  看\n  日志"}"#, None),
            Some(ToolRowTarget::Text("先 看 日志".to_owned()))
        );
    }
}
