//! Agent TUI slash 命令处理。

use axon_config::{REASONING_EFFORT_ARGUMENT_HINT, REASONING_EFFORT_VALUES};

use crate::r#loop::agent_tui::{
    AXON_VERSION, AgentTuiLifecyclePhase, AgentTuiMcpAuthAction, AgentTuiModelCatalogItem,
    AgentTuiTokenUsage,
    commands::{
        COMMAND_CLEAR, COMMAND_EXIT, COMMAND_HELP, COMMAND_MCP, COMMAND_MODEL, COMMAND_SKILLS,
        COMMAND_STATUS, COMMAND_THINK, MCP_AUTH_LOGIN_ARG, MCP_AUTH_LOGOUT_ARG, MCP_LIST_ARG,
        MCP_USAGE, command_available_during_task, is_known_command, parse_slash_command_token,
        similar_commands,
    },
    runtime_identity::display_model,
    state::AgentTuiState,
};
use crate::surface::cell::RenderStatusItem;

const AXON_REPOSITORY_URL: &str = env!("CARGO_PKG_REPOSITORY");
const SKILLS_ACTION_LIST: &str = "list";
const SKILLS_ACTION_TOGGLE: &str = "toggle";

pub(super) enum AgentTuiCommandOutcome {
    NotCommand,
    Handled,
}

impl AgentTuiState {
    pub(super) fn handle_command(&mut self, prompt: &str) -> AgentTuiCommandOutcome {
        let Some(command) = command_token_for_dispatch(prompt) else {
            return AgentTuiCommandOutcome::NotCommand;
        };
        let arg = prompt[command.len()..].trim();
        if self.is_working() && !command_available_during_task(command) {
            self.push_command_error(
                "命令暂不可用",
                format!("{command} 不能在当前任务运行中执行"),
                Some("等待当前任务结束后再执行该命令。".to_owned()),
            );
            return AgentTuiCommandOutcome::Handled;
        }
        match command {
            COMMAND_HELP => self.show_help(),
            COMMAND_CLEAR => {
                let header_hint = self.header.hint;
                self.clear();
                self.clear_draft();
                self.header.hint = header_hint;
                return AgentTuiCommandOutcome::Handled;
            }
            COMMAND_EXIT | "/quit" | ":q" => self.bridge.should_exit = true,
            COMMAND_MODEL => self.set_model(arg),
            COMMAND_THINK => self.set_reasoning_effort(arg),
            COMMAND_STATUS => self.show_status(),
            COMMAND_MCP => return self.show_mcp(arg),
            COMMAND_SKILLS => self.show_skills(arg),
            _ if prompt == "exit" || prompt == "quit" => self.bridge.should_exit = true,
            _ if command.starts_with('/') => {
                self.show_unknown_slash_command(command);
                return AgentTuiCommandOutcome::Handled;
            }
            _ => return AgentTuiCommandOutcome::NotCommand,
        }
        if !self.bottom_view_open() && !self.composer_popup_open() {
            self.clear_draft();
        }
        AgentTuiCommandOutcome::Handled
    }

    fn show_help(&mut self) {
        self.push_catalog_history(
            "命令",
            &[
                "/model".to_owned(),
                "/think".to_owned(),
                "/status".to_owned(),
                "/mcp".to_owned(),
                "/skills".to_owned(),
                "/clear".to_owned(),
                "/exit".to_owned(),
            ],
            "输入 / 查看命令",
        );
    }

    fn show_unknown_slash_command(&mut self, command: &str) {
        let suggestions = similar_commands(command)
            .into_iter()
            .filter(|candidate| is_known_command(candidate))
            .collect::<Vec<_>>();
        let mut details = vec!["输入 / 查看命令".to_owned()];
        if !suggestions.is_empty() {
            details.push(format!("可选：{}", suggestions.join("、")));
        }
        self.push_command_error("未知命令", command, details.first().cloned());
        if details.len() > 1 {
            self.push_history_item("建议", "命令", "输入", Some(details[1].clone()));
        }
    }

    fn set_model(&mut self, value: &str) {
        let model_labels = self.model_catalog_labels();
        if value.is_empty() {
            self.push_catalog_history("模型", &model_labels, "先运行 axon setup 或 axon config");
            return;
        }
        let Some(model) = self
            .header
            .catalog
            .models
            .iter()
            .find(|candidate| candidate.label == value)
            .or_else(|| {
                self.header
                    .catalog
                    .models
                    .iter()
                    .find(|candidate| candidate.matches(value))
            })
            .cloned()
        else {
            self.push_command_error(
                "模型不存在",
                value,
                Some("输入 /model 查看可用模型".to_owned()),
            );
            return;
        };
        self.header.overrides.model = Some(model.label.clone());
        self.header.config_status.model.clone_from(&model.label);
        self.record_settings_change();
    }

    fn model_catalog_labels(&self) -> Vec<String> {
        self.header
            .catalog
            .models
            .iter()
            .map(|model| model.label.clone())
            .collect()
    }

    fn set_reasoning_effort(&mut self, value: &str) {
        if !REASONING_EFFORT_VALUES.contains(&value) {
            self.push_usage_error(format!("/think {REASONING_EFFORT_ARGUMENT_HINT}"));
            return;
        }
        self.header.overrides.reasoning_effort = Some(value.to_owned());
        self.record_settings_change();
        self.push_status("设置", vec![command_status_item("Think", value)]);
    }

    fn show_status(&mut self) {
        self.push_status(
            "状态",
            vec![
                command_status_item("status.version", AXON_VERSION),
                command_status_item("status.repository", AXON_REPOSITORY_URL),
                command_status_item("status.model", self.status_model_summary()),
                command_status_item("status.provider", self.status_provider_summary()),
                command_status_item("status.directory", self.header.cwd.clone()),
                command_status_item("status.agents", self.status_agents_summary()),
                command_status_item("status.session", self.status_session_summary()),
                command_status_item("status.token_usage", self.status_token_usage_summary()),
                command_status_item(
                    "status.context_window",
                    self.status_context_window_summary(),
                ),
            ],
        );
    }

    fn reasoning_effort_status(&self) -> &str {
        self.header
            .overrides
            .reasoning_effort
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("medium")
    }

    fn status_model_summary(&self) -> String {
        format!(
            "{}（推理 {}）",
            display_model(&self.header.config_status, &self.header.overrides),
            self.reasoning_effort_status()
        )
    }

    fn status_provider_summary(&self) -> String {
        let base_url = self
            .header
            .config_status
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.name.starts_with("provider."))
            .and_then(|diagnostic| diagnostic.message.split(" · ").next())
            .filter(|value| !value.trim().is_empty());
        base_url.map_or_else(
            || self.header.config_status.provider.clone(),
            |url| format!("{} - {url}", self.header.config_status.provider),
        )
    }

    fn status_agents_summary(&self) -> String {
        match self.header.workspace_instruction_paths.as_slice() {
            [] => "未找到".to_owned(),
            [path] => format!("已加载 1 个：{path}"),
            paths => format!("已加载 {} 个：{}", paths.len(), paths[paths.len() - 1]),
        }
    }

    fn status_session_summary(&self) -> String {
        self.header
            .session_summary
            .session_id
            .clone()
            .or_else(|| self.header.overrides.session.clone())
            .unwrap_or_else(|| {
                if self.header.overrides.continue_last {
                    "last".to_owned()
                } else {
                    "new".to_owned()
                }
            })
    }

    fn status_token_usage_summary(&self) -> String {
        let usage = self.session_usage();
        format!(
            "{} 总计（{} 输入 + {} 输出 + {} 缓存写入 + {} 缓存读取{}）",
            usage.render_total(),
            usage.render_input(),
            usage.render_output(),
            usage.render_cache_write_input(),
            usage.render_cache_read_input(),
            status_reasoning_suffix(usage),
        )
    }

    fn status_context_window_summary(&self) -> String {
        let usage = self.session_usage();
        let used = usage.current_context_tokens();
        let Some(window) = self
            .current_model_catalog_item()
            .and_then(|model| model.context_window)
        else {
            return format!(
                "未知（已用 {}）",
                AgentTuiTokenUsage::render_token_count(used)
            );
        };
        if window == 0 {
            return format!(
                "未知（已用 {}）",
                AgentTuiTokenUsage::render_token_count(used)
            );
        }
        let left = window.saturating_sub(used);
        let left_percent = left.saturating_mul(100) / window;
        format!(
            "剩余 {}%（已用 {} / {}）",
            left_percent,
            AgentTuiTokenUsage::render_token_count(used),
            AgentTuiTokenUsage::render_token_count(window)
        )
    }

    pub(in crate::r#loop::agent_tui) fn current_model_catalog_item(
        &self,
    ) -> Option<&AgentTuiModelCatalogItem> {
        let effective = self
            .header
            .overrides
            .model
            .as_deref()
            .unwrap_or(self.header.config_status.model.as_str());
        let display = display_model(&self.header.config_status, &self.header.overrides);
        self.header
            .catalog
            .models
            .iter()
            .find(|candidate| candidate.label == display)
            .or_else(|| {
                self.header
                    .catalog
                    .models
                    .iter()
                    .find(|candidate| candidate.matches(effective))
            })
    }

    fn show_skills(&mut self, value: &str) {
        match value {
            "" => self.open_skills_menu(),
            SKILLS_ACTION_LIST => self.open_skills_list_from_view(),
            SKILLS_ACTION_TOGGLE => self.open_skills_toggle_view(),
            _ => {
                self.push_command_error("用法", "/skills [list|toggle]", None);
            }
        }
    }

    fn show_mcp(&mut self, value: &str) -> AgentTuiCommandOutcome {
        match value {
            "" | MCP_LIST_ARG => {
                self.request_catalog_reload(true);
                self.push_mcp_inventory();
            }
            _ => self.handle_mcp_auth_argument(value),
        }
        if !self.bottom_view_open() && !self.composer_popup_open() {
            self.clear_draft();
        }
        AgentTuiCommandOutcome::Handled
    }

    /// 处理 `/mcp login <server>` 与 `/mcp logout <server>`。
    ///
    /// 授权动作只登记请求，实际的浏览器授权与本地回调由主循环在后台线程执行，
    /// 避免阻塞 TUI 渲染与输入。
    fn handle_mcp_auth_argument(&mut self, value: &str) {
        let mut parts = value.split_whitespace();
        let action = match parts.next() {
            Some(MCP_AUTH_LOGIN_ARG) => AgentTuiMcpAuthAction::Login,
            Some(MCP_AUTH_LOGOUT_ARG) => AgentTuiMcpAuthAction::Logout,
            _ => {
                self.push_usage_error(MCP_USAGE);
                return;
            }
        };
        let Some(server_id) = parts.next() else {
            self.push_usage_error(MCP_USAGE);
            return;
        };
        if parts.next().is_some() {
            self.push_usage_error(MCP_USAGE);
            return;
        }
        if !self.mcp_server_known(server_id) {
            self.push_command_error(
                "MCP server 未找到",
                format!("当前 catalog 中没有 MCP server：{server_id}"),
                Some("运行 /mcp 查看可用 server".to_owned()),
            );
            return;
        }
        self.request_mcp_auth(action, server_id);
        self.push_status(
            format!("MCP {}", action.label()),
            vec![command_status_item(
                "mcp_auth",
                format!("{server_id} 的授权任务已提交，完成后自动刷新 /mcp 列表"),
            )],
        );
    }

    /// 判断 server 是否在 catalog 中；catalog 尚未加载时交由宿主校验。
    fn mcp_server_known(&self, server_id: &str) -> bool {
        self.header.catalog.mcp_servers.is_empty()
            || self
                .header
                .catalog
                .mcp_servers
                .iter()
                .any(|server| server.name == server_id)
    }

    fn push_command_error(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        next_step: Option<String>,
    ) {
        self.push_local_error(title, message, next_step);
    }

    pub(super) fn push_usage_error(&mut self, usage: impl Into<String>) {
        self.push_local_error("用法", usage, None);
    }
}

pub(super) fn command_status_item(
    kind: impl Into<String>,
    summary: impl Into<String>,
) -> RenderStatusItem {
    RenderStatusItem {
        kind: kind.into(),
        summary: summary.into(),
        phase: Some(AgentTuiLifecyclePhase::Command),
        indicator: None,
        tool_name: None,
        duration_ms: None,
        turn_id: None,
        attempt_index: None,
    }
}

fn status_reasoning_suffix(usage: AgentTuiTokenUsage) -> String {
    if usage.reasoning_output_tokens == 0 {
        return String::new();
    }
    format!(" + {} 推理", usage.render_reasoning_output())
}

fn command_token_for_dispatch(prompt: &str) -> Option<&str> {
    let trimmed = prompt.trim_start();
    if trimmed == "exit" || trimmed == "quit" || trimmed == ":q" {
        return Some(trimmed);
    }
    parse_slash_command_token(trimmed)
}
