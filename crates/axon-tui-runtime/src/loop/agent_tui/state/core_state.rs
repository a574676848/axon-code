//! Agent TUI 核心状态构造与只读访问器。

use crate::app_shell::header_state::HeaderState;
use crate::r#loop::agent_tui::state::command_handlers::command_status_item;
use crate::r#loop::agent_tui::state::toast::ToastState;
use crate::r#loop::agent_tui::state::visible_text_selection::VisibleTextSelectionState;
use crate::r#loop::agent_tui::{
    AgentTuiCatalog, AgentTuiCatalogLoadSnapshot, AgentTuiCatalogLoadState, AgentTuiLaunchRequest,
    AgentTuiMcpAuthAction, AgentTuiMcpAuthOutcome, AgentTuiMcpAuthRequest, AgentTuiPlanProgress,
    history_store,
    runtime_bridge::RuntimeBridge,
    startup_transcript::{startup_overview_for_catalog, startup_transcript},
};
use crate::surface::watchers::{RuntimeWatcherEvent, RuntimeWatcherPayload, RuntimeWatcherSource};
use crate::surface::{
    cell::{RenderCellData, RenderCellMode, RenderCellRecord},
    cell_store::CellStore,
};
use crate::ui_workflow::composer_state::ComposerState;
use ratatui::layout::Size;

use super::{AgentRuntimeStatus, AgentTuiState, ViewportState};

/// 首屏检测到 MCP 需要 OAuth 授权时的提示标题。
pub(in crate::r#loop::agent_tui) const MCP_AUTH_NOTICE_TITLE: &str = "MCP 需要授权";

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn new(request: &AgentTuiLaunchRequest) -> Self {
        let history_path = history_store::draft_history_path(&request.runtime_tmp_dir);
        let history = history_store::load_draft_history(&history_path);
        let plan_stream_controller = Some(
            crate::r#loop::agent_tui::streaming::controller::PlanStreamController::new(
                80,
                &request.cwd,
            ),
        );
        let mut state = Self {
            header: HeaderState::new(request),
            viewport: ViewportState::new(),
            composer: ComposerState::new(history, history_path),
            cells: CellStore::new(startup_transcript(request)),
            bridge: RuntimeBridge::new(request.runtime_tmp_dir.clone(), plan_stream_controller),
            visible_text_selection: VisibleTextSelectionState::default(),
            toast: ToastState::default(),
            clipboard: Default::default(),
            #[cfg(test)]
            test_runtime_root: None,
            terminal_session_projection: Default::default(),
        };
        if let Some(prompt) = request
            .prompt
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            state.composer.draft.replace_all(prompt.clone());
            state.refresh_command_suggestions();
        }
        state
    }

    #[cfg(test)]
    pub(in crate::r#loop::agent_tui) fn new_for_test(request: &AgentTuiLaunchRequest) -> Self {
        let runtime_root = std::sync::Arc::new(axon_test_support::test_temp_dir());
        let mut request = request.clone();
        request.runtime_tmp_dir = runtime_root.path().to_path_buf();
        let mut state = Self::new(&request);
        state.clipboard = super::clipboard::ClipboardBackend::Memory {
            text: None,
            image: None,
        };
        state.test_runtime_root = Some(runtime_root);
        state
    }

    pub(in crate::r#loop::agent_tui) fn plan_progress(&self) -> Option<AgentTuiPlanProgress> {
        self.bridge
            .live_plan_line
            .iter()
            .chain(self.cells.transcript.iter().rev())
            .find_map(|line| match &line.data {
                Some(RenderCellData::Plan(cell)) => cell.progress(),
                _ => None,
            })
    }

    /// 获取当前活跃的 LSP client 数量。
    pub(in crate::r#loop::agent_tui) const fn lsp_active_count(&self) -> usize {
        self.header.lsp_active_count
    }

    pub(in crate::r#loop::agent_tui) fn apply_catalog_load_snapshot(
        &mut self,
        snapshot: AgentTuiCatalogLoadSnapshot,
    ) {
        self.header.catalog.apply_load_snapshot(snapshot);
        self.refresh_command_suggestions();
        self.refresh_startup_overview();
        if self.cells.transcript_surfaces.catalog_cell("mcp").is_some() {
            self.push_mcp_inventory();
        }
        self.push_mcp_auth_notice();
    }

    /// 登记一次 TUI 内的 MCP OAuth 授权动作，由主循环交给 runtime adapter 执行。
    pub(in crate::r#loop::agent_tui) fn request_mcp_auth(
        &mut self,
        action: AgentTuiMcpAuthAction,
        server_id: impl Into<String>,
    ) {
        self.bridge.mcp_auth_request = Some(AgentTuiMcpAuthRequest {
            action,
            server_id: server_id.into(),
        });
    }

    /// 取出待执行的 MCP OAuth 授权请求。
    pub(in crate::r#loop::agent_tui) fn take_mcp_auth_request(
        &mut self,
    ) -> Option<AgentTuiMcpAuthRequest> {
        self.bridge.mcp_auth_request.take()
    }

    /// 应用 MCP OAuth 后台任务结果。
    ///
    /// 授权成功后必须强制刷新 catalog，否则加载失败留下的 MCP 健康负缓存会让
    /// `/mcp` 列表继续显示旧失败原因。
    pub(in crate::r#loop::agent_tui) fn apply_mcp_auth_outcome(
        &mut self,
        outcome: AgentTuiMcpAuthOutcome,
    ) {
        let label = outcome.action.label();
        match outcome.result {
            Ok(summary) => {
                self.push_status(
                    format!("MCP {label}"),
                    vec![command_status_item("mcp_auth", summary)],
                );
                self.request_catalog_reload(true);
            }
            Err(error) => self.push_local_error(
                format!("MCP {label}失败"),
                error,
                Some(format!(
                    "可重试 /mcp {action} {server}，或在终端运行 axon mcp {action} {server}",
                    action = mcp_auth_command(outcome.action),
                    server = outcome.server_id
                )),
            ),
        }
    }

    /// 宿主不支持 TUI 内授权时给出终端回退提示。
    pub(in crate::r#loop::agent_tui) fn fail_mcp_auth_request(
        &mut self,
        request: &AgentTuiMcpAuthRequest,
        message: impl Into<String>,
    ) {
        self.push_local_error(
            format!("MCP {}未执行", request.action.label()),
            message,
            Some(format!(
                "请在终端运行 axon mcp {action} {server}",
                action = mcp_auth_command(request.action),
                server = request.server_id
            )),
        );
    }

    /// 首屏 catalog 加载完成后提示一次需要 OAuth 登录的 MCP server。
    ///
    /// 探测复用 catalog 后台加载结果（含 MCP 健康负缓存），不额外发起网络请求，
    /// 也不会自动弹出浏览器。
    fn push_mcp_auth_notice(&mut self) {
        if self.bridge.mcp_auth_notice_shown {
            return;
        }
        let servers = mcp_auth_required_servers(&self.header.catalog);
        if servers.is_empty() {
            return;
        }
        self.bridge.mcp_auth_notice_shown = true;
        let commands = servers
            .iter()
            .map(|server| format!("/mcp login {server}"))
            .collect::<Vec<_>>()
            .join("、");
        self.push_notice_status(
            MCP_AUTH_NOTICE_TITLE,
            vec![command_status_item(
                "mcp_auth_required",
                format!(
                    "{} 需要 OAuth 登录；运行 {commands} 完成授权",
                    servers.join("、")
                ),
            )],
        );
    }

    pub(in crate::r#loop::agent_tui) fn request_catalog_reload(&mut self, force_mcp_refresh: bool) {
        self.bridge.catalog_reload_requested = true;
        self.bridge.catalog_reload_force_mcp_refresh |= force_mcp_refresh;
        self.header.catalog.mcp_state = AgentTuiCatalogLoadState::Loading;
        self.header.catalog.mcp_servers.clear();
        self.header.catalog.mcp_entries.clear();
        self.refresh_startup_overview();
    }

    pub(in crate::r#loop::agent_tui) fn take_catalog_reload_request(&mut self) -> Option<bool> {
        if !self.bridge.catalog_reload_requested {
            return None;
        }
        let force_mcp_refresh = self.bridge.catalog_reload_force_mcp_refresh;
        self.bridge.catalog_reload_requested = false;
        self.bridge.catalog_reload_force_mcp_refresh = false;
        Some(force_mcp_refresh)
    }

    pub(in crate::r#loop::agent_tui) fn catalog_reload_launch_request(
        &self,
    ) -> AgentTuiLaunchRequest {
        AgentTuiLaunchRequest {
            cwd: self.header.cwd.clone().into(),
            prompt: None,
            workspace_instruction_paths: self.header.workspace_instruction_paths.clone(),
            config_status: self.header.config_status.clone(),
            overrides: self.header.overrides.clone(),
            catalog: self.header.catalog.clone(),
            runtime_tmp_dir: self.bridge.runtime_tmp_dir.clone(),
        }
    }

    pub(in crate::r#loop::agent_tui) fn fail_catalog_reload(&mut self, message: impl Into<String>) {
        self.apply_catalog_load_snapshot(AgentTuiCatalogLoadSnapshot {
            skill_state: self.header.catalog.skill_state,
            skills: self.header.catalog.skills.clone(),
            skill_errors: self.header.catalog.skill_errors.clone(),
            mcp_state: AgentTuiCatalogLoadState::Failed,
            mcp_servers: Vec::new(),
            sessions: self.header.catalog.sessions.clone(),
        });
        self.push_local_error("MCP 重新加载失败", message.into(), None);
    }

    fn refresh_startup_overview(&mut self) {
        let next = RenderCellRecord::startup(startup_overview_for_catalog(&self.header.catalog));
        let startup_index = {
            self.cells
                .transcript
                .iter()
                .position(|line| line.mode == RenderCellMode::Startup)
        };
        if let Some(index) = startup_index {
            let _ = self.cells.transcript.replace_cell(index, next);
        }
    }

    pub(in crate::r#loop::agent_tui) fn apply_watcher_event(&mut self, event: RuntimeWatcherEvent) {
        self.bridge.record_watcher_event(&event);
        match event.payload {
            RuntimeWatcherPayload::Refresh(snapshot) => {
                self.request_catalog_reload(snapshot.source == RuntimeWatcherSource::Mcp);
            }
        }
    }

    pub(in crate::r#loop::agent_tui) fn is_working(&self) -> bool {
        self.bridge.runtime_status == AgentRuntimeStatus::Working
    }

    pub(in crate::r#loop::agent_tui) const fn runtime_submission_enabled(&self) -> bool {
        self.bridge.runtime_submission_enabled
    }

    pub(in crate::r#loop::agent_tui) fn set_frame_size(&mut self, size: Size) {
        let size = Size::new(size.width.max(1), size.height.max(1));
        if self.viewport.frame_size.width != size.width {
            let _primary_reflow = self
                .viewport
                .transcript_render_cache
                .compact_for_resize(size.width);
            self.viewport.transcript_height_cache.clear();
        }
        self.viewport.frame_size = size;
    }
}

/// 授权动作对应的 CLI 子命令名。
fn mcp_auth_command(action: AgentTuiMcpAuthAction) -> &'static str {
    match action {
        AgentTuiMcpAuthAction::Login => "login",
        AgentTuiMcpAuthAction::Logout => "logout",
    }
}

/// 收集 catalog 中明确需要 OAuth 授权的 MCP server。
fn mcp_auth_required_servers(catalog: &AgentTuiCatalog) -> Vec<String> {
    catalog
        .mcp_servers
        .iter()
        .filter(|server| {
            server
                .error
                .as_deref()
                .is_some_and(axon_agent_runtime::mcp_oauth_required)
        })
        .map(|server| server.name.clone())
        .collect()
}
