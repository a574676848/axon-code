//! Composer 建议面板与目录展示状态逻辑。

use super::super::{
    AgentTuiCatalog, AgentTuiCatalogLoadState, TUI_HINT_COMMAND, TUI_HINT_IDLE,
    commands::{AgentTuiCommandSuggestion, command_suggestions, command_supports_inline_args},
    state::AgentTuiState,
};
use crate::surface::{
    cell::RenderCellRecord,
    cell_event_data::{
        AgentRenderHistoryCell, AgentRenderLifecycleStatus, AgentRenderMcpCell, AgentRenderMcpItem,
    },
};

const CATALOG_KIND_MCP: &str = "mcp";
const CATALOG_KIND_HISTORY: &str = "history";
const MCP_INVENTORY_TITLE: &str = "MCP";
const MCP_INVENTORY_EMPTY_TEXT: &str = "当前没有已载入的 MCP server/tool";
const MCP_CATALOG_LOADING_LABEL: &str = "MCP catalog";
const MCP_CATALOG_FAILED_SUMMARY: &str = "MCP 加载失败";

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn complete_command_trigger(&mut self) {
        if self.composer.draft.text().trim().is_empty() {
            self.replace_draft("/".to_owned());
            return;
        }
        if let Some(selected) = self.selected_command_suggestion() {
            let command = selected.command.clone();
            let has_argument_hint = !selected.argument_hint.is_empty();
            if self.complete_inline_command(&command).is_none() {
                self.replace_draft(format!("{command} "));
            }
            if has_argument_hint {
                self.header.hint = TUI_HINT_COMMAND;
            }
        }
    }

    pub(in crate::r#loop::agent_tui) fn accept_selected_command_suggestion(&mut self) -> bool {
        let Some(selected) = self.selected_command_suggestion() else {
            return false;
        };
        let command = selected.command.clone();
        if self.complete_inline_command(&command).is_none() {
            self.replace_draft(format!("{command} "));
        }
        true
    }

    pub(in crate::r#loop::agent_tui) fn accept_selected_command_suggestion_for_enter(
        &mut self,
    ) -> bool {
        let Some(selected) = self.selected_command_suggestion() else {
            return true;
        };
        let command = selected.command.clone();
        let waits_for_argument =
            !selected.argument_hint.is_empty() && !command.contains(char::is_whitespace);
        if waits_for_argument {
            if let Some(has_inline_args) = self.complete_inline_command(&command) {
                return has_inline_args;
            }
            self.replace_draft(format!("{command} "));
            return false;
        }
        self.replace_draft(command);
        true
    }

    pub(super) fn selected_command_suggestion(&self) -> Option<&AgentTuiCommandSuggestion> {
        self.composer
            .command_suggestions
            .get(self.composer.selected_command_suggestion)
    }

    pub(in crate::r#loop::agent_tui) fn select_previous_command_suggestion(&mut self) {
        if self.composer.command_suggestions.is_empty() {
            return;
        }
        self.composer.selected_command_suggestion =
            if self.composer.selected_command_suggestion == 0 {
                self.composer.command_suggestions.len() - 1
            } else {
                self.composer.selected_command_suggestion - 1
            };
    }

    pub(in crate::r#loop::agent_tui) fn select_next_command_suggestion(&mut self) {
        if self.composer.command_suggestions.is_empty() {
            return;
        }
        self.composer.selected_command_suggestion = (self.composer.selected_command_suggestion + 1)
            % self.composer.command_suggestions.len();
    }

    pub(in crate::r#loop::agent_tui) fn command_suggestions_open(&self) -> bool {
        self.composer.composer_popup.is_none()
            && slash_command_input(self.composer.draft.text(), self.composer.draft.cursor())
                .is_some()
            && !self.composer.command_suggestions.is_empty()
    }

    pub(in crate::r#loop::agent_tui) fn command_suggestion_panel_height(&self) -> u16 {
        let suggestion_count = if let Some(popup) = self.composer.composer_popup.as_ref() {
            popup.items.len()
        } else if self.command_suggestions_open() {
            self.composer.command_suggestions.len()
        } else {
            0
        };
        if suggestion_count == 0 {
            return 0;
        }
        crate::widgets::CommandPalette::required_height(suggestion_count)
    }

    pub(super) fn refresh_command_suggestions(&mut self) {
        let command_input =
            slash_command_input(self.composer.draft.text(), self.composer.draft.cursor());
        if command_input.is_some() {
            self.composer.composer_popup = None;
            self.composer.path_completion.clear();
        } else {
            self.refresh_composer_popup();
        }
        self.composer.command_suggestions =
            command_input.as_deref().map_or_else(Vec::new, |input| {
                command_suggestions(input, &self.header.catalog, self.is_working())
            });
        self.composer.selected_command_suggestion = self
            .composer
            .selected_command_suggestion
            .min(self.composer.command_suggestions.len().saturating_sub(1));
        self.header.hint = if command_input.is_some() {
            TUI_HINT_COMMAND
        } else {
            TUI_HINT_IDLE
        };
    }

    fn complete_inline_command(&mut self, command: &str) -> Option<bool> {
        if !command_supports_inline_args(command) {
            return None;
        }
        let text = self.composer.draft.text();
        let cursor = self.composer.draft.cursor();
        let first_line_len = text
            .split_once('\n')
            .map_or_else(|| text.chars().count(), |(line, _)| line.chars().count());
        if cursor <= 1 || cursor > first_line_len || !text.starts_with('/') {
            return None;
        }
        let command_prefix = text.chars().take(cursor).collect::<String>();
        if command_prefix[1..].contains(char::is_whitespace) {
            return None;
        }
        let tail = text.chars().skip(cursor).collect::<String>();
        let has_inline_args = !tail.trim().is_empty();
        let replacement = if tail.chars().next().is_some_and(char::is_whitespace) {
            command.to_owned()
        } else {
            format!("{command} ")
        };
        self.replace_draft_range(0, cursor, &replacement);
        self.composer.draft.move_cursor_to_document_end();
        Some(has_inline_args)
    }

    pub(super) fn push_catalog_history(&mut self, title: &str, items: &[String], empty_text: &str) {
        let kind = catalog_cell_kind(title);
        let title_text = title.to_owned();
        let line = RenderCellRecord::history_with_data(AgentRenderHistoryCell::from_catalog_items(
            format!("history:catalog:{kind}"),
            title_text,
            kind,
            items,
            empty_text,
        ));
        self.upsert_catalog_surface(kind, line);
    }

    pub(super) fn push_mcp_inventory(&mut self) {
        let items = mcp_catalog_render_items(&self.header.catalog);
        let line = RenderCellRecord::mcp_with_data(AgentRenderMcpCell::from_catalog_items(
            "mcp:catalog:inventory",
            MCP_INVENTORY_TITLE,
            &items,
            MCP_INVENTORY_EMPTY_TEXT,
        ));
        self.upsert_catalog_surface(CATALOG_KIND_MCP, line);
    }

    fn upsert_catalog_surface(&mut self, kind: &str, line: RenderCellRecord) {
        if let Some(index) = self.cells.transcript_surfaces.catalog_cell(kind)
            && let Some(existing) = self.cells.transcript.get_mut(index)
        {
            existing.replace_with(line);
            return;
        }
        self.cells.transcript.push(line);
        self.cells.transcript_surfaces.set_catalog_cell(
            kind.to_owned(),
            self.cells.transcript.len().saturating_sub(1),
        );
    }
}

fn mcp_catalog_render_items(catalog: &AgentTuiCatalog) -> Vec<AgentRenderMcpItem> {
    if !catalog.mcp_servers.is_empty() {
        return catalog
            .mcp_servers
            .iter()
            .flat_map(|server| {
                if server.tools.is_empty() {
                    return vec![AgentRenderMcpItem::catalog(
                        server.name.clone(),
                        None,
                        lifecycle_status(server.status),
                        server.error.clone(),
                    )];
                }
                server
                    .tools
                    .iter()
                    .map(|tool| {
                        AgentRenderMcpItem::catalog(
                            server.name.clone(),
                            Some(tool.clone()),
                            lifecycle_status(server.status),
                            server.error.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    if catalog.mcp_state != AgentTuiCatalogLoadState::Loaded {
        return vec![AgentRenderMcpItem::catalog(
            MCP_CATALOG_LOADING_LABEL,
            None,
            lifecycle_status(catalog.mcp_state),
            (catalog.mcp_state == AgentTuiCatalogLoadState::Failed)
                .then(|| MCP_CATALOG_FAILED_SUMMARY.to_owned()),
        )];
    }
    catalog
        .mcp_entries
        .iter()
        .map(|entry| AgentRenderMcpItem::from_catalog_value(entry))
        .collect()
}

const fn lifecycle_status(state: AgentTuiCatalogLoadState) -> AgentRenderLifecycleStatus {
    match state {
        AgentTuiCatalogLoadState::Loading => AgentRenderLifecycleStatus::Running,
        AgentTuiCatalogLoadState::Loaded => AgentRenderLifecycleStatus::Completed,
        AgentTuiCatalogLoadState::Failed => AgentRenderLifecycleStatus::Failed,
    }
}

fn catalog_cell_kind(title: &str) -> &'static str {
    if title == "MCP" {
        CATALOG_KIND_MCP
    } else {
        CATALOG_KIND_HISTORY
    }
}

fn slash_command_input(text: &str, cursor: usize) -> Option<String> {
    let first_line_len = text
        .split_once('\n')
        .map_or_else(|| text.chars().count(), |(line, _)| line.chars().count());
    if cursor == 0 || cursor > first_line_len || !text.starts_with('/') {
        return None;
    }
    let prefix = text.chars().take(cursor).collect::<String>();
    Some(prefix.strip_prefix('/')?.to_owned())
}
