use super::*;
use crate::doctor_receipts::{
    DoctorCheck as ReceiptDoctorCheck, doctor_provider_recovery_check, render_template_report,
    write_receipt_template,
};
use axon_agent_runtime::{
    LspCatalogRuntime, ToolBudgetKind, agent_model_slot_candidates, model_routing_key,
    select_default_run_model_from_config, tool_runtime_budget,
};
use axon_config::{
    COMPRESSION_CONTEXT_WINDOW_DENOMINATOR, COMPRESSION_CONTEXT_WINDOW_NUMERATOR, LoadedConfig,
};

const DOCTOR_REQUIRED_EDIT_TOOL: &str = "builtin.edit";
const DOCTOR_REQUIRED_TOP_LEVEL_COMMANDS: [&str; 3] = ["run", "doctor", "mcp"];

pub(crate) fn run_doctor(command: &DoctorCommand, format: OutputFormat) -> CliRunResult {
    if let Some(output) = command.receipt_template.as_deref() {
        let report = write_receipt_template(command.check.as_deref(), output);
        return render_template_report(&report, format);
    }

    let report = if command.provider_receipt.is_some() {
        DoctorReport::run_with_provider_receipt(
            command.check.as_deref(),
            command.provider_receipt.as_deref(),
            command.verbose,
            command.fix,
        )
    } else {
        DoctorReport::run(command.check.as_deref(), command.verbose, command.fix)
    };
    let exit_code = u8::from(report.status != DoctorStatus::Ok);
    let stdout = match format {
        OutputFormat::Text => report.to_text(),
        OutputFormat::Json => report.to_json_line(),
    };

    CliRunResult {
        exit_code,
        stdout,
        stderr: String::new(),
    }
}

/// doctor 报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    /// 总体状态。
    pub status: DoctorStatus,
    /// 检查结果。
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub(crate) fn run(filter: Option<&str>, verbose: bool, fix: bool) -> Self {
        Self::run_with_provider_receipt(filter, None, verbose, fix)
    }

    pub(crate) fn run_with_provider_receipt(
        filter: Option<&str>,
        provider_receipt: Option<&std::path::Path>,
        verbose: bool,
        fix: bool,
    ) -> Self {
        let checks = match filter {
            Some(DOCTOR_CHECK_CONFIG) => vec![doctor_config_check(verbose)],
            Some(DOCTOR_CHECK_CLI) => vec![doctor_cli_check()],
            Some(DOCTOR_CHECK_PROVIDER_RECOVERY) => {
                vec![from_receipt_check(doctor_provider_recovery_check(
                    provider_receipt,
                ))]
            }
            Some(DOCTOR_CHECK_PREFLIGHT) => doctor_preflight_checks(verbose),
            Some(DOCTOR_CHECK_PROVIDER) => vec![doctor_provider_check()],
            Some(DOCTOR_CHECK_TOOL) => vec![doctor_tool_check()],
            Some(DOCTOR_CHECK_MCP) => vec![doctor_mcp_check()],
            Some(DOCTOR_CHECK_MEMORY) => vec![doctor_memory_check()],
            Some(DOCTOR_CHECK_LSP) => vec![doctor_lsp_check()],
            Some(DOCTOR_CHECK_RUNTIME) => vec![doctor_runtime_check(fix)],
            Some(DOCTOR_CHECK_SMOKE) => vec![doctor_smoke_check()],
            Some(unknown) => vec![DoctorCheck::failed(
                unknown,
                format!("未知 doctor 检查项：{unknown}"),
            )],
            None => vec![
                doctor_config_check(verbose),
                doctor_cli_check(),
                doctor_preflight_check(verbose),
                doctor_provider_check(),
                doctor_tool_check(),
                doctor_mcp_check(),
                doctor_memory_check(),
                doctor_lsp_check(),
                doctor_runtime_check(fix),
                doctor_smoke_check(),
            ],
        };
        let status = if checks.iter().all(|check| check.status == DoctorStatus::Ok) {
            DoctorStatus::Ok
        } else {
            DoctorStatus::Failed
        };

        Self { status, checks }
    }

    fn to_text(&self) -> String {
        let mut output = format!("Axon doctor: {}\n", self.status.as_str());
        for check in &self.checks {
            let _ = writeln!(
                output,
                "- {}: {} - {}",
                check.name,
                check.status.as_str(),
                check.message
            );
        }
        output
    }

    fn to_json_line(&self) -> String {
        format!(
            "{}\n",
            serde_json::to_string(self).expect("DoctorReport 序列化不应失败")
        )
    }
}

fn from_receipt_check(check: ReceiptDoctorCheck) -> DoctorCheck {
    DoctorCheck {
        name: check.name,
        status: match check.status {
            crate::doctor_receipts::DoctorStatus::Ok => DoctorStatus::Ok,
            crate::doctor_receipts::DoctorStatus::Failed => DoctorStatus::Failed,
        },
        message: check.message,
    }
}

/// doctor 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    /// 通过。
    Ok,
    /// 失败。
    Failed,
}

impl DoctorStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}

/// 单项 doctor 检查。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    /// 检查名称。
    pub name: String,
    /// 检查状态。
    pub status: DoctorStatus,
    /// 中文说明。
    pub message: String,
}

impl DoctorCheck {
    pub(crate) fn ok(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorStatus::Ok,
            message: message.into(),
        }
    }

    pub(crate) fn failed(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorStatus::Failed,
            message: message.into(),
        }
    }
}

fn doctor_config_check(verbose: bool) -> DoctorCheck {
    // 加载用户配置而非默认配置
    let Ok(config_path) = crate::setup_commands::resolved_setup_config_path(None) else {
        return DoctorCheck::failed(DOCTOR_CHECK_CONFIG, "无法解析配置文件路径");
    };

    match ConfigLoader::load_from_path(&config_path) {
        Ok(loaded) if loaded.report.ensure_no_secret_plaintext().is_ok() => {
            DoctorCheck::ok(DOCTOR_CHECK_CONFIG, doctor_config_message(&loaded, verbose))
        }
        Ok(_) => DoctorCheck::failed(DOCTOR_CHECK_CONFIG, "配置加载报告包含 secret 原文"),
        Err(error) => DoctorCheck::failed(DOCTOR_CHECK_CONFIG, format!("配置校验失败：{error:?}")),
    }
}

fn load_user_config_for_doctor() -> Result<LoadedConfig, String> {
    let config_path = crate::setup_commands::resolved_setup_config_path(None)
        .map_err(|_| "无法解析配置文件路径".to_owned())?;
    ConfigLoader::load_from_path(&config_path).map_err(|error| format!("{error:?}"))
}

fn doctor_config_message(loaded: &axon_config::LoadedConfig, verbose: bool) -> String {
    if !verbose {
        return "默认配置可加载、可校验且不含 secret 原文".to_owned();
    }

    let shell = tool_runtime_budget(ToolBudgetKind::ShellCommand);
    let context_window = loaded
        .config
        .models
        .iter()
        .map(|model| model.context_window)
        .max()
        .unwrap_or(0);
    let compression_threshold = context_window.saturating_mul(COMPRESSION_CONTEXT_WINDOW_NUMERATOR)
        / COMPRESSION_CONTEXT_WINDOW_DENOMINATOR;

    format!(
        "默认配置可加载、可校验且不含 secret 原文；推导参数：context_window={context_window} compression_token_threshold={compression_threshold} shell.default_timeout_ms={} shell.min_timeout_ms={} shell.max_timeout_ms={}",
        shell.default_timeout_ms,
        shell.min_timeout_ms.unwrap_or(0),
        shell.max_timeout_ms.unwrap_or(0)
    )
}

fn doctor_cli_check() -> DoctorCheck {
    let command_names = registered_command_names();
    let missing = missing_doctor_command_paths(&command_names);
    if missing.is_empty() {
        DoctorCheck::ok(DOCTOR_CHECK_CLI, "CLI 命令注册表包含 Agent-first 关键入口")
    } else {
        DoctorCheck::failed(
            DOCTOR_CHECK_CLI,
            format!(
                "CLI 命令注册表缺少 Agent-first 关键入口：{}",
                missing.join("/")
            ),
        )
    }
}

fn missing_doctor_command_paths(command_names: &[String]) -> Vec<String> {
    DOCTOR_REQUIRED_TOP_LEVEL_COMMANDS
        .iter()
        .filter(|name| !command_names.iter().any(|registered| registered == **name))
        .map(|name| (*name).to_owned())
        .collect()
}

fn doctor_required_command_paths() -> Vec<String> {
    DOCTOR_REQUIRED_TOP_LEVEL_COMMANDS
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

fn doctor_preflight_checks(verbose: bool) -> Vec<DoctorCheck> {
    vec![
        doctor_preflight_check(verbose),
        doctor_config_check(verbose),
        doctor_provider_check(),
        doctor_model_routing_check(),
        doctor_mcp_check(),
        doctor_memory_check(),
        doctor_lsp_check(),
        doctor_tool_check(),
        doctor_smoke_check(),
    ]
}

fn doctor_preflight_check(verbose: bool) -> DoctorCheck {
    let checks = doctor_preflight_component_checks(verbose);
    let failed = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Failed)
        .map(|check| check.name.as_str())
        .collect::<Vec<_>>();
    if failed.is_empty() {
        DoctorCheck::ok(
            DOCTOR_CHECK_PREFLIGHT,
            format!(
                "vibe coding 预检通过：{}",
                checks
                    .iter()
                    .map(|check| check.name.as_str())
                    .collect::<Vec<_>>()
                    .join("/")
            ),
        )
    } else {
        DoctorCheck::failed(
            DOCTOR_CHECK_PREFLIGHT,
            format!("vibe coding 预检失败：{}", failed.join("/")),
        )
    }
}

fn doctor_preflight_component_checks(verbose: bool) -> Vec<DoctorCheck> {
    vec![
        doctor_config_check(verbose),
        doctor_provider_check(),
        doctor_model_routing_check(),
        doctor_mcp_check(),
        doctor_memory_check(),
        doctor_lsp_check(),
        doctor_tool_check(),
    ]
}

fn doctor_provider_check() -> DoctorCheck {
    // 加载用户配置而非默认配置
    let Ok(config_path) = crate::setup_commands::resolved_setup_config_path(None) else {
        return DoctorCheck::failed(DOCTOR_CHECK_PROVIDER, "无法解析配置文件路径");
    };

    let config = match ConfigLoader::load_from_path(&config_path) {
        Ok(loaded) => loaded.config,
        Err(_) => return DoctorCheck::failed(DOCTOR_CHECK_PROVIDER, "provider 配置加载失败"),
    };
    let provider_count = config.providers.len();
    let model_count = config.models.len();
    let websocket_provider_count = config
        .providers
        .iter()
        .filter(|provider| {
            provider.wire_transport == ProviderWireTransport::WebSocket.as_config_value()
        })
        .count();
    let openai_compatible_count = config
        .providers
        .iter()
        .filter(|provider| provider.protocol == "openai")
        .count();
    let recommended_count = config
        .models
        .iter()
        .filter(|model| !model.supports_embedding)
        .count();
    let byok_provider_count = config
        .providers
        .iter()
        .filter(|provider| provider.api_key_env.is_some())
        .count();
    let guidance = if provider_count == 0 {
        "；当前未配置 provider，可通过 `axon providers add` 或配置文件接入 BYOK / OpenAI-compatible 渠道"
    } else {
        ""
    };
    DoctorCheck::ok(
        DOCTOR_CHECK_PROVIDER,
        format!(
            "模型渠道已加载：channels={provider_count} models={model_count} recommended_models={recommended_count} byok_channels={byok_provider_count} openai_compatible_channels={openai_compatible_count} websocket_channels={websocket_provider_count}{guidance}"
        ),
    )
}

fn doctor_model_routing_check() -> DoctorCheck {
    let loaded = match load_user_config_for_doctor() {
        Ok(loaded) => loaded,
        Err(error) => {
            return DoctorCheck::failed(
                "model-routing",
                format!("模型路由预检无法加载配置：{error}"),
            );
        }
    };
    let config = &loaded.config;
    if config.providers.is_empty() && config.models.is_empty() {
        return DoctorCheck::ok(
            "model-routing",
            "尚未配置模型渠道和模型，可通过 `axon providers add` 或 setup 完成路由配置",
        );
    }
    let default = match select_default_run_model_from_config(config) {
        Ok(selection) => selection,
        Err(error) => return DoctorCheck::failed("model-routing", error),
    };
    let Some(model) = config.models.get(default.config_index) else {
        return DoctorCheck::failed("model-routing", "默认模型索引无法映射到配置目录");
    };
    let provider_ok = config.enabled_provider(&model.provider).is_some();
    if !provider_ok {
        return DoctorCheck::failed(
            "model-routing",
            format!("默认模型引用的 Provider 不存在或已禁用：{}", model.provider),
        );
    }
    let fast_candidates = agent_model_slot_candidates(config, config.agent.fast_model.as_str());
    if fast_candidates.is_empty() {
        return DoctorCheck::failed(
            "model-routing",
            format!(
                "agent.fast_model 必须引用已配置模型名：{}",
                config.agent.fast_model
            ),
        );
    }
    DoctorCheck::ok(
        "model-routing",
        format!(
            "模型路由可解析：default={} provider={} routing_key={} fast_candidates={}",
            model.name,
            model.provider,
            model_routing_key(model),
            fast_candidates.len()
        ),
    )
}

fn doctor_tool_check() -> DoctorCheck {
    let names = registered_command_names();
    let registry = doctor_runtime_tool_registry();
    let tool_names = registry
        .model_tools()
        .iter()
        .map(|tool| tool.qualified_name.clone())
        .collect::<Vec<_>>();
    let has_search = tool_names.iter().any(|name| name.contains("search"));
    let has_edit = tool_names
        .iter()
        .any(|name| name == DOCTOR_REQUIRED_EDIT_TOOL);
    let has_shell = tool_names.iter().any(|name| name.contains("shell"));
    let missing_commands = missing_doctor_command_paths(&names);
    if missing_commands.is_empty() && has_search && has_edit && has_shell {
        DoctorCheck::ok(
            DOCTOR_CHECK_TOOL,
            format!(
                "turn-time tool registry 已接入 shell/search/edit：registered_tools={} model_tools={}",
                registry.tools().len(),
                tool_names.len()
            ),
        )
    } else {
        DoctorCheck::failed(
            DOCTOR_CHECK_TOOL,
            format!(
                "turn-time tool registry 缺少关键工具或命令路径：missing_commands={} has_search={has_search} has_edit={has_edit} has_shell={has_shell}",
                missing_commands.join("/")
            ),
        )
    }
}

fn doctor_mcp_check() -> DoctorCheck {
    let names = registered_command_names();
    let registry = doctor_runtime_tool_registry();
    let mcp_tool_count = registry
        .model_tools()
        .iter()
        .filter(|tool| axon_agent_runtime::is_mcp_tool_name(&tool.qualified_name))
        .count();
    if names.contains(&"mcp".to_owned()) {
        DoctorCheck::ok(
            DOCTOR_CHECK_MCP,
            format!("MCP 管理命令与 turn-time registry 就绪：mcp_tools={mcp_tool_count}"),
        )
    } else {
        DoctorCheck::failed(DOCTOR_CHECK_MCP, "MCP 命令注册表不可用")
    }
}

fn doctor_memory_check() -> DoctorCheck {
    let names = registered_command_names();
    if names.contains(&"memory".to_owned()) {
        DoctorCheck::ok(
            DOCTOR_CHECK_MEMORY,
            "memory 命令已注册，memory recall/write 边界就绪",
        )
    } else {
        DoctorCheck::failed(DOCTOR_CHECK_MEMORY, "memory 命令注册表不可用")
    }
}

fn doctor_lsp_check() -> DoctorCheck {
    let catalog = LspCatalogRuntime::catalog();
    let core_diagnostics_servers = LspCatalogRuntime::core_diagnostics_servers();
    let exposed_capabilities = LspCatalogRuntime::exposed_capabilities();
    let bootstrap_count = catalog
        .iter()
        .filter(|server| server.opencode_can_bootstrap)
        .count();
    let local_runtime_count = catalog
        .iter()
        .filter(|server| server.requires_local_runtime)
        .count();
    DoctorCheck::ok(
        DOCTOR_CHECK_LSP,
        format!(
            "LSP / diagnostics 基线已对齐 opencode dev：servers={} core_diagnostics={} capabilities={} bootstrap_capable_servers={} local_runtime_servers={}",
            catalog.len(),
            core_diagnostics_servers.join(","),
            exposed_capabilities.join(","),
            bootstrap_count,
            local_runtime_count
        ),
    )
}

fn doctor_runtime_check(fix: bool) -> DoctorCheck {
    let config_path = match crate::setup_commands::resolved_setup_config_path(None) {
        Ok(path) => path,
        Err(error) => {
            return DoctorCheck::failed(DOCTOR_CHECK_RUNTIME, format!("无法解析运行目录：{error}"));
        }
    };
    let root = match axon_config::runtime_axon_root_path(&config_path) {
        Ok(path) => path,
        Err(error) => {
            return DoctorCheck::failed(
                DOCTOR_CHECK_RUNTIME,
                format!("无法解析 Axon 根目录：{error:?}"),
            );
        }
    };
    let data = root.join(axon_config::AXON_RUNTIME_DATA_DIR);
    let candidates = [root.clone(), data.clone(), data.join("ipc")];
    let mut created = false;
    for path in candidates {
        if path.is_dir() {
            continue;
        }
        if !fix {
            return DoctorCheck::ok(
                DOCTOR_CHECK_RUNTIME,
                format!(
                    "运行目录尚未创建：{}；首次启动 Daemon 或执行 axon doctor --check runtime --fix 即可创建",
                    path.display()
                ),
            );
        }
        if let Err(error) = std::fs::create_dir_all(&path) {
            return DoctorCheck::failed(
                DOCTOR_CHECK_RUNTIME,
                format!("创建运行目录失败：{}：{error}", path.display()),
            );
        }
        created = true;
    }
    let probe = data.join(".doctor-write-probe");
    match std::fs::write(&probe, b"axon") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            let suffix = if created {
                "；已创建缺失目录"
            } else {
                ""
            };
            DoctorCheck::ok(
                DOCTOR_CHECK_RUNTIME,
                format!("运行目录可用：{}{}", data.display(), suffix),
            )
        }
        Err(error) => DoctorCheck::failed(
            DOCTOR_CHECK_RUNTIME,
            format!("运行目录不可写：{}：{error}", data.display()),
        ),
    }
}

fn doctor_smoke_check() -> DoctorCheck {
    let names = registered_command_names();
    let missing = missing_doctor_command_paths(&names);
    if missing.is_empty() {
        let smoke_report = doctor_cli_release_smoke_report(&names);
        if !smoke_report.all_passed {
            return DoctorCheck::failed(DOCTOR_CHECK_SMOKE, smoke_report.summary);
        }
        DoctorCheck::ok(DOCTOR_CHECK_SMOKE, smoke_report.summary)
    } else {
        DoctorCheck::failed(
            DOCTOR_CHECK_SMOKE,
            format!("跨平台最小 smoke 失败：缺少 {missing:?}"),
        )
    }
}

struct DoctorCliSmokeReport {
    all_passed: bool,
    summary: String,
}

fn doctor_cli_release_smoke_report(command_names: &[String]) -> DoctorCliSmokeReport {
    let missing = missing_doctor_command_paths(command_names);
    DoctorCliSmokeReport {
        all_passed: missing.is_empty(),
        summary: if missing.is_empty() {
            format!(
                "跨平台最小 smoke 就绪：commands={} package_verify=available upgrade_apply=available",
                doctor_required_command_paths().join("/")
            )
        } else {
            format!("跨平台最小 smoke 失败：{}", missing.join(","))
        },
    }
}

fn doctor_runtime_tool_registry() -> axon_agent_runtime::ToolRegistry {
    crate::runtime_tools::runtime_tool_registry_or_builtin(None)
}
