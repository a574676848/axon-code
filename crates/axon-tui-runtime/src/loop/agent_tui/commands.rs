use std::collections::BTreeSet;

use axon_config::{REASONING_EFFORT_ARGUMENT_HINT, REASONING_EFFORT_VALUES};

use super::{AgentTuiCatalog, AgentTuiModelCatalogItem, AgentTuiTokenUsage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentTuiCommandSuggestion {
    pub command: String,
    pub description: String,
    pub argument_hint: String,
}

struct AgentTuiCommandTemplate {
    command: &'static str,
    description: &'static str,
    argument_hint: &'static str,
    available_during_task: bool,
}

pub(super) const COMMAND_HELP: &str = "/help";
pub(super) const COMMAND_CLEAR: &str = "/clear";
pub(super) const COMMAND_EXIT: &str = "/exit";
pub(super) const COMMAND_MODEL: &str = "/model";
pub(super) const COMMAND_THINK: &str = "/think";
pub(super) const COMMAND_STATUS: &str = "/status";
pub(super) const COMMAND_MCP: &str = "/mcp";
pub(super) const COMMAND_SKILLS: &str = "/skills";
pub(super) const COMMAND_SESSIONS: &str = "/sessions";
pub(super) const COMMAND_NEW: &str = "/new";
pub(super) const COMMAND_RESET: &str = "/reset";

/// `/sessions` 的用法提示。
pub(super) const SESSIONS_USAGE: &str = "/sessions [--all]";
/// `/sessions --all`：把归档会话一并列出。
pub(super) const SESSIONS_ALL_ARG: &str = "--all";

/// `/mcp` 用法提示。
pub(super) const MCP_USAGE: &str = "/mcp [list|login|logout <server>]";
/// MCP 列表子命令。
pub(super) const MCP_LIST_ARG: &str = "list";
/// MCP OAuth 登录子命令。
pub(super) const MCP_AUTH_LOGIN_ARG: &str = "login";
/// MCP OAuth 登出子命令。
pub(super) const MCP_AUTH_LOGOUT_ARG: &str = "logout";

const MCP_AUTH_ACTIONS: [&str; 2] = [MCP_AUTH_LOGIN_ARG, MCP_AUTH_LOGOUT_ARG];
/// `/mcp ` 之后每个动作的补充说明：动作名本身不含「做什么」，这里给出结论。
const MCP_ACTION_HINTS: [(&str, &str); 3] = [
    (MCP_LIST_ARG, "列出 server 与工具"),
    (MCP_AUTH_LOGIN_ARG, "OAuth 登录"),
    (MCP_AUTH_LOGOUT_ARG, "登出并清除 token"),
];

const COMMANDS: [AgentTuiCommandTemplate; 11] = [
    command(COMMAND_HELP, "查看 TUI 能力和常用命令", "", true),
    command(COMMAND_CLEAR, "清空当前会话视图，保留顶部状态栏", "", false),
    command(
        COMMAND_SESSIONS,
        "打开会话弹窗，选中后载入该会话历史",
        "[--all]",
        false,
    ),
    command(
        COMMAND_NEW,
        "开启新会话，下一轮使用新的 session id",
        "",
        false,
    ),
    command(
        COMMAND_RESET,
        "重置当前会话的运行实例，保留 session id 与历史",
        "",
        false,
    ),
    command(COMMAND_EXIT, "退出 Axon Agent TUI", "", true),
    command(COMMAND_MODEL, "切换下一轮使用的模型", "<model>", false),
    command(
        COMMAND_THINK,
        "设置推理强度",
        REASONING_EFFORT_ARGUMENT_HINT,
        false,
    ),
    command(COMMAND_STATUS, "查看当前 TUI 运行状态", "", true),
    command(
        COMMAND_MCP,
        "查看 MCP server/tool 列表或完成 OAuth 授权",
        "[login|logout <server>]",
        true,
    ),
    command(COMMAND_SKILLS, "管理 skills", "[list|toggle]", true),
];

const MAX_DYNAMIC_SUGGESTIONS: usize = 50;

pub(super) const EXIT_COMMAND_ALIASES: &[&str] = &["/quit", ":q", "exit", "quit"];

#[cfg(test)]
pub(super) fn builtin_command_tokens() -> impl Iterator<Item = &'static str> {
    COMMANDS.iter().map(|item| item.command)
}

pub(super) fn is_known_command(command: &str) -> bool {
    EXIT_COMMAND_ALIASES.contains(&command) || COMMANDS.iter().any(|item| item.command == command)
}

pub(super) fn parse_slash_command_token(prompt: &str) -> Option<&str> {
    let trimmed = prompt.trim_start();
    if !trimmed.starts_with('/') {
        return None;
    }
    let token = trimmed.split_whitespace().next().unwrap_or_default();
    let name = token.strip_prefix('/').unwrap_or_default();
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some(token)
}

pub(super) fn similar_commands(command: &str) -> Vec<&'static str> {
    let name = command.trim_start_matches('/').to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter_map(|item| {
            let candidate = item.command.trim_start_matches('/');
            (candidate.starts_with(&name)
                || name.starts_with(candidate)
                || candidate.contains(&name)
                || name.contains(candidate)
                || edit_distance_at_most_two(candidate, &name))
            .then_some(item.command)
        })
        .take(3)
        .collect()
}

fn edit_distance_at_most_two(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let len_delta = left.len().abs_diff(right.len());
    if len_delta > 2 {
        return false;
    }
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_byte) in left.iter().enumerate() {
        current[0] = left_index + 1;
        let mut row_min = current[0];
        for (right_index, right_byte) in right.iter().enumerate() {
            let substitution_cost = usize::from(left_byte != right_byte);
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + substitution_cost);
            row_min = row_min.min(current[right_index + 1]);
        }
        if row_min > 2 {
            return false;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()] <= 2
}

const fn command(
    command: &'static str,
    description: &'static str,
    argument_hint: &'static str,
    available_during_task: bool,
) -> AgentTuiCommandTemplate {
    AgentTuiCommandTemplate {
        command,
        description,
        argument_hint,
        available_during_task,
    }
}

pub(super) fn command_suggestions(
    input: &str,
    catalog: &AgentTuiCatalog,
    task_running: bool,
) -> Vec<AgentTuiCommandSuggestion> {
    let normalized = input.trim_start_matches('/').trim_start();
    let Some((command_filter, argument_filter)) = split_command_input(normalized) else {
        return builtin_command_suggestions(normalized, task_running);
    };
    let command_name = format!("/{command_filter}");
    if task_running && !command_available_during_task(&command_name) {
        return Vec::new();
    }
    let argument_suggestions = argument_suggestions(&command_name, argument_filter, catalog);
    if argument_suggestions.is_empty() && is_known_command(&command_name) {
        return Vec::new();
    }
    if argument_suggestions.is_empty() {
        return builtin_command_suggestions(command_filter, task_running);
    }
    argument_suggestions
}

pub(super) fn command_available_during_task(command: &str) -> bool {
    COMMANDS
        .iter()
        .find(|item| item.command == command)
        .is_some_and(|item| item.available_during_task)
        || EXIT_COMMAND_ALIASES.contains(&command)
}

pub(super) fn command_supports_inline_args(command: &str) -> bool {
    let _ = command;
    false
}

/// 参数提示是否可省略：整段被方括号包住即视为可选。
///
/// `/sessions [--all]` 这类命令裸跑就成立：用户打完命令直接回车应当执行，
/// 而不是被补全改写一次、再按第二次。必填参数的提示不带方括号（如 `<path>`）。
pub(super) fn argument_hint_is_optional(hint: &str) -> bool {
    let hint = hint.trim();
    hint.starts_with('[') && hint.ends_with(']')
}

fn builtin_command_suggestions(filter: &str, task_running: bool) -> Vec<AgentTuiCommandSuggestion> {
    COMMANDS
        .iter()
        .filter(|item| !task_running || item.available_during_task)
        .filter(|item| item.command.trim_start_matches('/').starts_with(filter))
        .map(|item| AgentTuiCommandSuggestion {
            command: item.command.to_owned(),
            description: item.description.to_owned(),
            argument_hint: item.argument_hint.to_owned(),
        })
        .collect()
}

fn split_command_input(input: &str) -> Option<(&str, &str)> {
    let command_end = input.find(char::is_whitespace)?;
    let command = &input[..command_end];
    let argument = input[command_end..].trim_start();
    Some((command, argument))
}

fn argument_suggestions(
    command_name: &str,
    argument_filter: &str,
    catalog: &AgentTuiCatalog,
) -> Vec<AgentTuiCommandSuggestion> {
    match command_name {
        COMMAND_MODEL => model_suggestions(argument_filter, catalog),
        COMMAND_MCP => mcp_suggestions(argument_filter, catalog),
        COMMAND_SESSIONS => static_value_suggestions(
            COMMAND_SESSIONS,
            argument_filter,
            &[(SESSIONS_ALL_ARG, "连归档会话一起列出")],
        ),
        COMMAND_SKILLS => static_value_suggestions(
            COMMAND_SKILLS,
            argument_filter,
            &[("list", "列出已载入技能"), ("toggle", "开关指定技能")],
        ),
        // 强度名本身就说清了动作：逐条重复同一句「设置思考强度」只是噪声。
        COMMAND_THINK => static_value_suggestions(
            COMMAND_THINK,
            argument_filter,
            &bare_values(REASONING_EFFORT_VALUES),
        ),
        _ => Vec::new(),
    }
}

fn model_suggestions(filter: &str, catalog: &AgentTuiCatalog) -> Vec<AgentTuiCommandSuggestion> {
    let mut seen = BTreeSet::new();
    catalog
        .models
        .iter()
        .filter(|model| value_matches(filter, &model.label))
        .filter(|model| seen.insert(model.label.to_ascii_lowercase()))
        .take(MAX_DYNAMIC_SUGGESTIONS)
        .map(|model| AgentTuiCommandSuggestion {
            command: format!("{COMMAND_MODEL} {}", model.label),
            // 左列已经写着模型名：右列改成左列没有的事实，provider 与窗口大小。
            description: model_facts(model),
            argument_hint: model.label.clone(),
        })
        .collect()
}

/// 模型弹窗右列的有效信息：provider 与上下文窗口；缺哪一项就只写另一项。
fn model_facts(model: &AgentTuiModelCatalogItem) -> String {
    let provider = model.provider.trim();
    let window = model
        .context_window
        .filter(|window| *window > 0)
        .map(|window| format!("{} 窗口", AgentTuiTokenUsage::render_token_count(window)));
    match (provider.is_empty(), window) {
        (false, Some(window)) => format!("{provider} · {window}"),
        (false, None) => provider.to_owned(),
        (true, Some(window)) => window,
        (true, None) => String::new(),
    }
}

fn mcp_suggestions(
    argument_filter: &str,
    catalog: &AgentTuiCatalog,
) -> Vec<AgentTuiCommandSuggestion> {
    let Some((action, server_filter)) = split_command_input(argument_filter) else {
        return static_value_suggestions(COMMAND_MCP, argument_filter, &MCP_ACTION_HINTS);
    };
    if !MCP_AUTH_ACTIONS.contains(&action) {
        return Vec::new();
    }
    let server_filter = server_filter.trim();
    catalog
        .mcp_servers
        .iter()
        .filter(|server| value_matches(server_filter, &server.name))
        .take(MAX_DYNAMIC_SUGGESTIONS)
        .map(|server| AgentTuiCommandSuggestion {
            command: format!("{COMMAND_MCP} {action} {}", server.name),
            // 左列已经带着 server 名，右列只留动作含义。
            description: mcp_action_label(action).to_owned(),
            argument_hint: server.name.clone(),
        })
        .collect()
}

/// MCP 授权子命令的中文动作名。
fn mcp_action_label(action: &str) -> &'static str {
    match action {
        MCP_AUTH_LOGIN_ARG => "登录授权",
        _ => "登出并清除 token",
    }
}

/// 把「取值 + 补充说明」投影成候选；说明为空时该行只留命令本身。
fn static_value_suggestions(
    command_name: &str,
    filter: &str,
    values: &[(&str, &str)],
) -> Vec<AgentTuiCommandSuggestion> {
    values
        .iter()
        .copied()
        .filter(|(value, _)| value_matches(filter, value))
        .take(MAX_DYNAMIC_SUGGESTIONS)
        .map(|(value, description)| AgentTuiCommandSuggestion {
            command: format!("{command_name} {value}"),
            description: description.to_owned(),
            argument_hint: value.to_owned(),
        })
        .collect()
}

/// 只有取值、没有补充说明的候选：取值本身就说清了动作。
fn bare_values<'a>(values: &'a [&'a str]) -> Vec<(&'a str, &'a str)> {
    values.iter().map(|value| (*value, "")).collect()
}

fn value_matches(filter: &str, candidate: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let filter = filter.to_ascii_lowercase();
    candidate.to_ascii_lowercase().contains(&filter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_argument_suggestions_do_not_require_verbose_mode() {
        let catalog = AgentTuiCatalog {
            mcp_entries: vec!["gitnexus".to_owned()],
            ..AgentTuiCatalog::default()
        };

        let suggestions = command_suggestions("mcp v", &catalog, false);

        assert!(suggestions.is_empty());
    }

    #[test]
    fn mcp_argument_suggestions_include_list_action() {
        let suggestions = command_suggestions("mcp l", &AgentTuiCatalog::default(), false);

        assert!(
            suggestions
                .iter()
                .any(|suggestion| suggestion.command == "/mcp list")
        );
    }

    #[test]
    fn skills_argument_suggestions_use_management_actions_not_skill_names() {
        let catalog = AgentTuiCatalog {
            skills: vec!["review".to_owned()],
            ..AgentTuiCatalog::default()
        };

        let suggestions = command_suggestions("skills r", &catalog, false);

        assert!(
            suggestions
                .iter()
                .all(|suggestion| suggestion.command != "/skills review")
        );
    }

    #[test]
    fn model_argument_suggestions_dedupe_by_display_label() {
        let catalog = AgentTuiCatalog {
            models: vec![
                super::super::AgentTuiModelCatalogItem::new("claude-sonnet-4-6", "upstream-a", "a"),
                super::super::AgentTuiModelCatalogItem::new("claude-sonnet-4-6", "upstream-b", "b"),
                super::super::AgentTuiModelCatalogItem::new("gpt-5.5", "gpt-5.5", "openai"),
            ],
            ..AgentTuiCatalog::default()
        };

        let suggestions = command_suggestions("model ", &catalog, false);

        assert_eq!(
            suggestions
                .iter()
                .filter(|suggestion| suggestion.argument_hint == "claude-sonnet-4-6")
                .count(),
            1
        );
        assert!(
            suggestions
                .iter()
                .all(|suggestion| suggestion.command.starts_with("/model "))
        );
    }

    #[test]
    fn model_suggestions_describe_facts_the_left_column_does_not_show() {
        let catalog = AgentTuiCatalog {
            models: vec![
                super::super::AgentTuiModelCatalogItem::new_with_context_window(
                    "claude-sonnet-4-6",
                    "upstream-a",
                    "anthropic",
                    Some(200_000),
                ),
                super::super::AgentTuiModelCatalogItem::new("gpt-5.5", "gpt-5.5", "openai"),
            ],
            ..AgentTuiCatalog::default()
        };

        let suggestions = command_suggestions("model ", &catalog, false);
        let model = &suggestions[0];
        let plain = &suggestions[1];

        assert_eq!(model.description, "anthropic · 200K 窗口");
        assert_eq!(plain.description, "openai");
        for suggestion in suggestions {
            assert!(
                !suggestion.description.contains(&suggestion.argument_hint),
                "右列不许复述左列已经写着的模型名：{}",
                suggestion.description
            );
        }
    }

    #[test]
    fn think_suggestions_do_not_repeat_one_identical_description() {
        let suggestions = command_suggestions("think ", &AgentTuiCatalog::default(), false);

        assert!(suggestions.len() > 1);
        assert!(
            suggestions
                .iter()
                .all(|suggestion| suggestion.description.is_empty()),
            "强度名本身就说清了动作，逐条重复同一句说明只是噪声"
        );
    }

    #[test]
    fn session_mutating_commands_are_unavailable_while_task_is_running() {
        let suggestions = command_suggestions("", &AgentTuiCatalog::default(), true);

        for command in [
            COMMAND_CLEAR,
            COMMAND_MODEL,
            COMMAND_THINK,
            COMMAND_SESSIONS,
            COMMAND_NEW,
            COMMAND_RESET,
        ] {
            assert!(!command_available_during_task(command));
            assert!(
                suggestions
                    .iter()
                    .all(|suggestion| suggestion.command != command)
            );
        }
    }

    #[test]
    fn session_switch_commands_are_known_and_completable() {
        let suggestions = command_suggestions("", &AgentTuiCatalog::default(), false);
        let commands = suggestions
            .iter()
            .map(|suggestion| suggestion.command.as_str())
            .collect::<Vec<_>>();

        for command in [COMMAND_SESSIONS, COMMAND_NEW, COMMAND_RESET] {
            assert!(is_known_command(command), "{command} 必须可被识别");
            assert!(commands.contains(&command), "{command} 必须出现在补全里");
        }
    }

    #[test]
    fn sessions_argument_suggestion_offers_archived_flag() {
        let suggestions = command_suggestions("sessions ", &AgentTuiCatalog::default(), false);

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].command, "/sessions --all");
        assert_eq!(suggestions[0].argument_hint, SESSIONS_ALL_ARG);
    }

    #[test]
    fn similar_commands_recovers_reset_typo() {
        for (typo, expected) in [
            ("/resett", COMMAND_RESET),
            ("/sesions", COMMAND_SESSIONS),
            ("/neww", COMMAND_NEW),
        ] {
            assert!(
                similar_commands(typo).contains(&expected),
                "{typo} 应提示 {expected}"
            );
        }
    }

    #[test]
    fn think_suggestions_cover_official_provider_union() {
        let suggestions = command_suggestions("think ", &AgentTuiCatalog::default(), false);
        let values = suggestions
            .iter()
            .map(|suggestion| suggestion.argument_hint.as_str())
            .collect::<Vec<_>>();

        assert_eq!(values, REASONING_EFFORT_VALUES);
        assert!(!values.contains(&"ultra"));
    }
}
