use std::collections::BTreeSet;

use axon_config::{REASONING_EFFORT_ARGUMENT_HINT, REASONING_EFFORT_VALUES};

use super::AgentTuiCatalog;

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

/// `/mcp` 用法提示。
pub(super) const MCP_USAGE: &str = "/mcp [list|login|logout <server>]";
/// MCP 列表子命令。
pub(super) const MCP_LIST_ARG: &str = "list";
/// MCP OAuth 登录子命令。
pub(super) const MCP_AUTH_LOGIN_ARG: &str = "login";
/// MCP OAuth 登出子命令。
pub(super) const MCP_AUTH_LOGOUT_ARG: &str = "logout";

const MCP_AUTH_ACTIONS: [&str; 2] = [MCP_AUTH_LOGIN_ARG, MCP_AUTH_LOGOUT_ARG];
const MCP_ACTIONS: [&str; 3] = [MCP_LIST_ARG, MCP_AUTH_LOGIN_ARG, MCP_AUTH_LOGOUT_ARG];

const COMMANDS: [AgentTuiCommandTemplate; 8] = [
    command(COMMAND_HELP, "查看 TUI 能力和常用命令", "", true),
    command(COMMAND_CLEAR, "清空当前会话视图，保留顶部状态栏", "", false),
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
        COMMAND_SKILLS => static_value_suggestions(
            COMMAND_SKILLS,
            "Skill 管理",
            argument_filter,
            &["list", "toggle"],
        ),
        COMMAND_THINK => static_value_suggestions(
            COMMAND_THINK,
            "设置思考强度",
            argument_filter,
            REASONING_EFFORT_VALUES,
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
            description: format!("切换模型：{}", model.label),
            argument_hint: model.label.clone(),
        })
        .collect()
}

fn mcp_suggestions(
    argument_filter: &str,
    catalog: &AgentTuiCatalog,
) -> Vec<AgentTuiCommandSuggestion> {
    let Some((action, server_filter)) = split_command_input(argument_filter) else {
        return static_value_suggestions(COMMAND_MCP, "MCP 操作", argument_filter, &MCP_ACTIONS);
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
            description: format!("MCP {}：{}", mcp_action_label(action), server.name),
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

fn static_value_suggestions(
    command_name: &str,
    description: &str,
    filter: &str,
    values: &[&str],
) -> Vec<AgentTuiCommandSuggestion> {
    values
        .iter()
        .copied()
        .filter(|value| value_matches(filter, value))
        .take(MAX_DYNAMIC_SUGGESTIONS)
        .map(|value| AgentTuiCommandSuggestion {
            command: format!("{command_name} {value}"),
            description: description.to_owned(),
            argument_hint: value.to_owned(),
        })
        .collect()
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
    fn session_mutating_commands_are_unavailable_while_task_is_running() {
        let suggestions = command_suggestions("", &AgentTuiCatalog::default(), true);

        for command in [COMMAND_CLEAR, COMMAND_MODEL, COMMAND_THINK] {
            assert!(!command_available_during_task(command));
            assert!(
                suggestions
                    .iter()
                    .all(|suggestion| suggestion.command != command)
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
