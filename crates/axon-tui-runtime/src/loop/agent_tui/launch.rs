//! Agent TUI 启动契约与报告。

use std::{collections::BTreeSet, path::PathBuf};

use serde::Serialize;

use super::ability_matrix::{AgentTuiAbilityMatrixSnapshot, agent_tui_ability_matrix_snapshot};
use super::smoke_contract::{AgentTuiSmokeContractSnapshot, agent_tui_smoke_contract_snapshot};
use super::{
    AGENT_TUI_FOCUS_TARGET, AGENT_TUI_KIND, AGENT_TUI_PANEL_TITLE, AGENT_TUI_PROMPT_INDICATOR,
    AGENT_TUI_STATUS_STARTED, AgentTuiSessionSummary, AgentTuiTokenUsage,
};
use crate::surface::{
    telemetry::TuiTelemetrySnapshot,
    terminal_capabilities::{TerminalCompatibilityPolicy, TerminalCompatibilitySnapshot},
};

/// Agent TUI 配置状态快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiConfigStatus {
    /// Provider 名称或配置域。
    pub provider: String,
    /// 模型名称或配置状态。
    pub model: String,
    /// API key / 配置可用性状态。
    pub api_key_status: String,
    /// 本次启动解析到的配置文件路径。
    pub config_path: Option<String>,
    /// 与配置文件相邻的 env 密钥文件路径。
    pub env_path: Option<String>,
    /// 当前配置的真实诊断摘要。
    pub diagnostics: Vec<AgentTuiConfigDiagnostic>,
}

/// Agent TUI 配置诊断项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiConfigDiagnostic {
    /// 诊断项名称。
    pub name: String,
    /// 诊断状态。
    pub status: AgentTuiConfigDiagnosticStatus,
    /// 用户可读摘要。
    pub message: String,
    /// 可选修复建议。
    pub remediation: Option<String>,
}

/// Agent TUI 配置诊断状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTuiConfigDiagnosticStatus {
    /// 通过。
    Ok,
    /// 警告。
    Warning,
    /// 失败。
    Failed,
}

/// Agent TUI 运行态可切换选项。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiRunOverrides {
    /// 当前 TUI 会话覆盖的模型策略或模型别名。
    pub model: Option<String>,
    /// 当前 TUI 会话 reasoning effort。
    pub reasoning_effort: Option<String>,
    /// 当前 TUI 会话绑定的 session。
    pub session: Option<String>,
    /// 是否继续最近会话。
    pub continue_last: bool,
}

/// Agent TUI 运行设置变更，用于由宿主 CLI 写回项目配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiSettingsChange {
    /// 当前选择的模型名称。
    pub model: Option<String>,
    /// 当前选择的 reasoning effort。
    pub reasoning_effort: Option<String>,
    /// 当前 TUI 会话变更的 skill 启用状态。
    pub skill_states: Vec<AgentTuiSkillStateChange>,
}

impl AgentTuiSettingsChange {
    /// 从运行覆盖值创建可持久化设置快照。
    #[must_use]
    pub fn from_overrides(overrides: &AgentTuiRunOverrides) -> Self {
        Self {
            model: overrides.model.clone(),
            reasoning_effort: overrides.reasoning_effort.clone(),
            skill_states: Vec::new(),
        }
    }

    /// 从 skill 启用状态变更创建可持久化设置快照。
    #[must_use]
    pub fn from_skill_states(skill_states: Vec<AgentTuiSkillStateChange>) -> Self {
        Self {
            model: None,
            reasoning_effort: None,
            skill_states,
        }
    }
}

/// 单个 skill 的启用状态变更。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiSkillStateChange {
    /// Skill 名称。
    pub name: String,
    /// 是否启用。
    pub enabled: bool,
}

/// Agent TUI catalog 后台加载状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTuiCatalogLoadState {
    /// 后台加载中。
    Loading,
    /// 已完成加载。
    #[default]
    Loaded,
    /// 加载失败。
    Failed,
}

/// MCP server 在 TUI catalog 中的结构化快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiMcpServerCatalogItem {
    /// Server 展示名。
    pub name: String,
    /// 当前加载状态。
    pub status: AgentTuiCatalogLoadState,
    /// 已加载 tool 名称。
    pub tools: Vec<String>,
    /// 失败原因或诊断摘要。
    pub error: Option<String>,
}

impl AgentTuiMcpServerCatalogItem {
    /// 创建已加载 server 快照。
    #[must_use]
    pub fn loaded(name: impl Into<String>, tools: Vec<String>) -> Self {
        Self {
            name: name.into(),
            status: AgentTuiCatalogLoadState::Loaded,
            tools,
            error: None,
        }
    }

    /// 创建加载失败 server 快照。
    #[must_use]
    pub fn failed(name: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: AgentTuiCatalogLoadState::Failed,
            tools: Vec::new(),
            error: Some(error.into()),
        }
    }
}

/// Agent TUI 后台 catalog 加载结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentTuiCatalogLoadSnapshot {
    /// skill 名称。
    pub skills: Vec<String>,
    /// skill 加载状态。
    pub skill_state: AgentTuiCatalogLoadState,
    /// skill 加载错误摘要。
    pub skill_errors: Vec<String>,
    /// MCP server 快照。
    pub mcp_servers: Vec<AgentTuiMcpServerCatalogItem>,
    /// MCP 加载状态。
    pub mcp_state: AgentTuiCatalogLoadState,
    /// 最近 session 元数据。
    pub sessions: Vec<AgentTuiSessionCatalogItem>,
}

/// TUI 最近会话目录项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiSessionCatalogItem {
    /// Session ID。
    pub session_id: String,
    /// 模型生成的会话标题。
    pub title: Option<String>,
    /// Provider 名称。
    pub provider: String,
    /// 模型名称。
    pub model: String,
    /// 更新时间。
    pub updated_at_ms: u64,
}

impl AgentTuiSessionCatalogItem {
    /// 创建会话目录项。
    #[must_use]
    pub fn new(
        session_id: impl Into<String>,
        title: Option<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        updated_at_ms: u64,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            title: title.and_then(|value| {
                let title = value.trim();
                (!title.is_empty()).then(|| title.to_owned())
            }),
            provider: provider.into(),
            model: model.into(),
            updated_at_ms,
        }
    }

    /// 展示标签。
    #[must_use]
    pub fn label(&self) -> &str {
        self.title.as_deref().unwrap_or(self.session_id.as_str())
    }
}

/// Agent TUI 可用目录。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentTuiCatalog {
    /// 模型项。
    pub models: Vec<AgentTuiModelCatalogItem>,
    /// provider 名称。
    pub providers: Vec<String>,
    /// skill 名称。
    pub skills: Vec<String>,
    /// skill 加载状态。
    pub skill_state: AgentTuiCatalogLoadState,
    /// skill 加载错误摘要。
    pub skill_errors: Vec<String>,
    /// 被禁用的 skill 名称。
    pub disabled_skills: BTreeSet<String>,
    /// connector 名称。
    pub connectors: Vec<String>,
    /// MCP server 结构化加载快照。
    pub mcp_servers: Vec<AgentTuiMcpServerCatalogItem>,
    /// MCP 加载状态。
    pub mcp_state: AgentTuiCatalogLoadState,
    /// MCP server 或工具名称。
    pub mcp_entries: Vec<String>,
    /// 最近 session 元数据。
    pub sessions: Vec<AgentTuiSessionCatalogItem>,
}

impl AgentTuiCatalog {
    /// 应用后台 catalog 加载快照。
    pub fn apply_load_snapshot(&mut self, snapshot: AgentTuiCatalogLoadSnapshot) {
        self.skills = snapshot.skills;
        self.skill_state = snapshot.skill_state;
        self.skill_errors = snapshot.skill_errors;
        self.mcp_servers = snapshot.mcp_servers;
        self.mcp_state = snapshot.mcp_state;
        self.sessions = snapshot.sessions;
        self.mcp_entries = self
            .mcp_servers
            .iter()
            .flat_map(|server| {
                server
                    .tools
                    .iter()
                    .map(|tool| format!("{}/{}", server.name, tool))
                    .collect::<Vec<_>>()
            })
            .collect();
    }

    /// 已加载 MCP tool 数量。
    #[must_use]
    pub fn loaded_mcp_tool_count(&self) -> usize {
        self.mcp_servers
            .iter()
            .map(|server| server.tools.len())
            .sum::<usize>()
            .max(self.mcp_entries.len())
    }

    /// 加载失败的 MCP server 数量。
    #[must_use]
    pub fn failed_mcp_server_count(&self) -> usize {
        self.mcp_servers
            .iter()
            .filter(|server| server.status == AgentTuiCatalogLoadState::Failed)
            .count()
    }

    /// 可显示的 MCP 加载摘要。
    #[must_use]
    pub fn mcp_summary_label(&self) -> String {
        match self.mcp_state {
            AgentTuiCatalogLoadState::Loading => "加载中".to_owned(),
            AgentTuiCatalogLoadState::Failed => {
                format!("失败 {}", self.failed_mcp_server_count().max(1))
            }
            AgentTuiCatalogLoadState::Loaded => {
                let failed = self.failed_mcp_server_count();
                if failed == 0 {
                    self.loaded_mcp_tool_count().to_string()
                } else {
                    format!("{} · 失败 {failed}", self.loaded_mcp_tool_count())
                }
            }
        }
    }

    /// 可显示的 Skill 加载摘要。
    #[must_use]
    pub fn skill_summary_label(&self) -> String {
        match self.skill_state {
            AgentTuiCatalogLoadState::Loading => "加载中".to_owned(),
            AgentTuiCatalogLoadState::Failed => {
                format!("失败 {}", self.skill_errors.len().max(1))
            }
            AgentTuiCatalogLoadState::Loaded => self.skills.len().to_string(),
        }
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    #[test]
    fn model_catalog_item_carries_declared_vision_capability() {
        assert!(AgentTuiModelCatalogItem::new("a", "a", "p").supports_vision);
        assert!(
            !AgentTuiModelCatalogItem::new("b", "b", "p")
                .with_supports_vision(false)
                .supports_vision
        );
    }

    #[test]
    fn catalog_load_snapshot_replaces_session_metadata() {
        let mut catalog = AgentTuiCatalog {
            sessions: vec![AgentTuiSessionCatalogItem::new(
                "old-session",
                Some("旧标题".to_owned()),
                "old",
                "model",
                1,
            )],
            ..AgentTuiCatalog::default()
        };

        catalog.apply_load_snapshot(AgentTuiCatalogLoadSnapshot {
            sessions: vec![AgentTuiSessionCatalogItem::new(
                "new-session",
                Some("新标题".to_owned()),
                "provider",
                "gpt",
                2,
            )],
            ..AgentTuiCatalogLoadSnapshot::default()
        });

        assert_eq!(catalog.sessions.len(), 1);
        assert_eq!(catalog.sessions[0].session_id, "new-session");
        assert_eq!(catalog.sessions[0].label(), "新标题");
    }
}

/// Agent TUI 可切换模型项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiModelCatalogItem {
    /// 展示给用户的模型名称。
    pub label: String,
    /// 运行时模型名称，作为渠道负载均衡路由键。
    pub selector: String,
    /// provider 名称。
    pub provider: String,
    /// 模型 context window，上游配置未知时为空。
    pub context_window: Option<u64>,
    /// 模型默认 reasoning effort，上游配置未知或未启用时为空。
    pub reasoning_effort: Option<String>,
    /// 模型是否声明支持视觉输入；提交含图片附件的草稿前必须据此预检。
    pub supports_vision: bool,
}

impl AgentTuiModelCatalogItem {
    /// 创建模型目录项。
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        selector: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            selector: selector.into(),
            provider: provider.into(),
            context_window: None,
            reasoning_effort: None,
            supports_vision: true,
        }
    }

    /// 创建带 context window 的模型目录项。
    #[must_use]
    pub fn new_with_context_window(
        label: impl Into<String>,
        selector: impl Into<String>,
        provider: impl Into<String>,
        context_window: Option<u64>,
    ) -> Self {
        Self::new_with_runtime_defaults(label, selector, provider, context_window, None)
    }

    /// 创建带运行默认值的模型目录项。
    #[must_use]
    pub fn new_with_runtime_defaults(
        label: impl Into<String>,
        selector: impl Into<String>,
        provider: impl Into<String>,
        context_window: Option<u64>,
        reasoning_effort: Option<String>,
    ) -> Self {
        Self {
            label: label.into(),
            selector: selector.into(),
            provider: provider.into(),
            context_window,
            reasoning_effort,
            supports_vision: true,
        }
    }

    /// 声明该模型的视觉输入能力，供提交前预检使用。
    #[must_use]
    pub fn with_supports_vision(mut self, supports_vision: bool) -> Self {
        self.supports_vision = supports_vision;
        self
    }

    /// 判断用户输入是否命中当前模型项。
    #[must_use]
    pub fn matches(&self, value: &str) -> bool {
        self.label == value
            || self.selector == value
            || format!("{}/{}", self.provider, self.label) == value
            || format!("{}/{}", self.provider, self.selector) == value
    }
}

/// Agent TUI 工作区状态快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiWorkspaceStatus {
    /// 当前 Git 分支或 detached HEAD 短提交。
    pub branch: String,
    /// 工作区是否存在未提交变更。
    pub dirty: bool,
}

impl AgentTuiWorkspaceStatus {
    /// 创建工作区状态快照。
    #[must_use]
    pub fn new(branch: impl Into<String>, dirty: bool) -> Self {
        Self {
            branch: branch.into(),
            dirty,
        }
    }
}

/// Agent TUI 启动请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiLaunchRequest {
    /// 工作目录。
    pub cwd: PathBuf,
    /// 初始 prompt。
    pub prompt: Option<String>,
    /// 启动时解析到的 workspace instruction 路径快照。
    pub workspace_instruction_paths: Vec<String>,
    /// 配置状态。
    pub config_status: AgentTuiConfigStatus,
    /// TUI 初始运行态覆盖。
    pub overrides: AgentTuiRunOverrides,
    /// TUI 命令提示目录。
    pub catalog: AgentTuiCatalog,
    /// 用户级 Axon 临时目录，默认应为 `<axon-root>/data/tmp`。
    pub runtime_tmp_dir: PathBuf,
}

/// Agent TUI 启动/退出报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiLaunchReport {
    /// 状态。
    pub status: &'static str,
    /// 类型。
    pub kind: &'static str,
    /// 工作目录。
    pub cwd: String,
    /// 初始 prompt。
    pub prompt: Option<String>,
    /// 从 composer 提交的 prompt。
    pub submitted_prompt: Option<String>,
    /// 当前 TUI 会话累计 token usage。
    pub session_usage: AgentTuiTokenUsage,
    /// 当前 TUI 会话运行摘要。
    pub session_summary: AgentTuiSessionSummary,
    /// 面板标题。
    pub panel_title: &'static str,
    /// 当前焦点。
    pub focus_target: &'static str,
    /// prompt 标识。
    pub prompt_indicator: &'static str,
    /// 配置状态。
    pub config_status: AgentTuiConfigStatus,
    /// TUI 运行态覆盖。
    pub overrides: AgentTuiRunOverrides,
    /// TUI 可观测性快照。
    pub telemetry: TuiTelemetrySnapshot,
    /// 当前终端兼容能力快照。
    pub terminal_compatibility: TerminalCompatibilitySnapshot,
    /// TUI 能力保留矩阵快照。
    pub ability_matrix: AgentTuiAbilityMatrixSnapshot,
    /// TUI 真机 smoke 目标态契约快照。
    pub smoke_contract: AgentTuiSmokeContractSnapshot,
    /// 用户可读消息。
    pub message: String,
}

impl AgentTuiConfigStatus {
    /// 创建配置状态快照。
    #[must_use]
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        api_key_status: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            api_key_status: api_key_status.into(),
            config_path: None,
            env_path: None,
            diagnostics: Vec::new(),
        }
    }

    /// 附加配置来源路径。
    #[must_use]
    pub fn with_paths(
        mut self,
        config_path: impl Into<String>,
        env_path: impl Into<String>,
    ) -> Self {
        self.config_path = Some(config_path.into());
        self.env_path = Some(env_path.into());
        self
    }

    /// 附加配置诊断摘要。
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: Vec<AgentTuiConfigDiagnostic>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    /// 创建可恢复的配置告警。
    #[must_use]
    pub fn warning(scope: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(scope, "需要配置", message)
    }
}

impl AgentTuiConfigDiagnostic {
    /// 创建诊断项。
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        status: AgentTuiConfigDiagnosticStatus,
        message: impl Into<String>,
        remediation: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status,
            message: message.into(),
            remediation,
        }
    }

    /// 渲染为单行中文诊断。
    #[must_use]
    pub fn render_line(&self) -> String {
        let remediation = self
            .remediation
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map_or(String::new(), |value| format!(" · 建议：{value}"));
        format!(
            "{}={} · {}{}",
            self.name,
            self.status.as_str(),
            self.message,
            remediation
        )
    }
}

impl AgentTuiConfigDiagnosticStatus {
    /// 中文友好的稳定状态值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Failed => "failed",
        }
    }
}

/// 生成不进入全屏的启动报告，供 JSON/smoke 使用。
#[must_use]
pub fn launch_agent_tui_report(request: &AgentTuiLaunchRequest) -> AgentTuiLaunchReport {
    AgentTuiLaunchReport {
        status: AGENT_TUI_STATUS_STARTED,
        kind: AGENT_TUI_KIND,
        cwd: request.cwd.to_string_lossy().to_string(),
        prompt: request.prompt.clone(),
        submitted_prompt: None,
        session_usage: AgentTuiTokenUsage::default(),
        session_summary: AgentTuiSessionSummary::default(),
        panel_title: AGENT_TUI_PANEL_TITLE,
        focus_target: AGENT_TUI_FOCUS_TARGET,
        prompt_indicator: AGENT_TUI_PROMPT_INDICATOR,
        config_status: request.config_status.clone(),
        overrides: request.overrides.clone(),
        telemetry: TuiTelemetrySnapshot::default(),
        terminal_compatibility: TerminalCompatibilityPolicy::current().snapshot(),
        ability_matrix: agent_tui_ability_matrix_snapshot(),
        smoke_contract: agent_tui_smoke_contract_snapshot(),
        message: "Axon Agent TUI 已进入默认交互入口".to_owned(),
    }
}
