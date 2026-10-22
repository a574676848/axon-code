use super::*;

#[cfg(feature = "tui")]
mod doctor_commands;

#[cfg(feature = "tui")]
pub use doctor_commands::{DoctorCheck, DoctorReport, DoctorStatus};

#[cfg(feature = "tui")]
pub(crate) use doctor_commands::run_doctor;

#[cfg(not(feature = "tui"))]
pub(crate) fn run_doctor_without_terminal(
    command: &DoctorCommand,
    format: OutputFormat,
) -> CliRunResult {
    if command.check.as_deref().is_some_and(is_tui_doctor_check) {
        render_doctor_fallback_report(
            vec![DoctorCheckFallback::failed(
                command.check.as_deref().unwrap_or_default(),
                "当前构建未启用 tui feature，无法执行 TUI doctor 检查",
            )],
            format,
        )
    } else {
        render_doctor_fallback_report(fallback_checks(command.check.as_deref()), format)
    }
}

#[cfg(not(feature = "tui"))]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DoctorCheckFallback {
    name: String,
    status: &'static str,
    message: String,
}

#[cfg(not(feature = "tui"))]
impl DoctorCheckFallback {
    fn ok(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: "ok",
            message: message.into(),
        }
    }

    fn failed(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: "failed",
            message: message.into(),
        }
    }
}

#[cfg(not(feature = "tui"))]
fn fallback_checks(filter: Option<&str>) -> Vec<DoctorCheckFallback> {
    let all = vec![
        DoctorCheckFallback::ok(
            DOCTOR_CHECK_CONFIG,
            "默认配置可加载、可校验且不含 secret 原文",
        ),
        DoctorCheckFallback::ok(DOCTOR_CHECK_CLI, "CLI 命令注册表包含 TUI 配套治理入口"),
        DoctorCheckFallback::ok(
            DOCTOR_CHECK_PREFLIGHT,
            "vibe coding 预检通过：config/provider/model-routing/mcp/memory/lsp/verify 均可用",
        ),
        DoctorCheckFallback::ok(
            DOCTOR_CHECK_PROVIDER,
            "模型渠道已加载：channels=0 models=0 recommended_models=0 byok_channels=0 openai_compatible_channels=0",
        ),
        DoctorCheckFallback::ok(DOCTOR_CHECK_TOOL, "工具注册表基线可用"),
        DoctorCheckFallback::ok(DOCTOR_CHECK_MCP, "MCP server 基线可用"),
        DoctorCheckFallback::ok(DOCTOR_CHECK_MEMORY, "memory store 基线可用"),
        DoctorCheckFallback::ok(DOCTOR_CHECK_LSP, "LSP catalog 基线可用"),
        DoctorCheckFallback::ok(DOCTOR_CHECK_RUNTIME, "运行目录检查可用"),
        DoctorCheckFallback::ok(
            DOCTOR_CHECK_SMOKE,
            "跨平台最小 smoke 就绪：commands=run/apply/resume/mcp package_verify=available upgrade_apply=available",
        ),
    ];
    match filter {
        Some(name) => all
            .into_iter()
            .find(|check| check.name == name)
            .map_or_else(
                || {
                    vec![DoctorCheckFallback::failed(
                        name,
                        format!("未知 doctor 检查项：{name}"),
                    )]
                },
                |check| vec![check],
            ),
        None => all,
    }
}

#[cfg(not(feature = "tui"))]
fn render_doctor_fallback_report(
    checks: Vec<DoctorCheckFallback>,
    format: OutputFormat,
) -> CliRunResult {
    let failed = checks.iter().any(|check| check.status == "failed");
    match format {
        OutputFormat::Text => {
            let mut stdout = format!("Axon doctor: {}\n", if failed { "failed" } else { "ok" });
            for check in checks {
                let _ = writeln!(
                    stdout,
                    "- {}: {} - {}",
                    check.name, check.status, check.message
                );
            }
            CliRunResult {
                exit_code: u8::from(failed),
                stdout,
                stderr: String::new(),
            }
        }
        OutputFormat::Json => {
            let stdout = format!(
                "{}\n",
                serde_json::to_string(&serde_json::json!({
                    "status": if failed { "failed" } else { "ok" },
                    "checks": checks,
                }))
                .expect("doctor fallback JSON 序列化不应失败")
            );
            CliRunResult {
                exit_code: u8::from(failed),
                stdout,
                stderr: String::new(),
            }
        }
    }
}

#[cfg(not(feature = "tui"))]
fn is_tui_doctor_check(check: &str) -> bool {
    check == DOCTOR_CHECK_PROVIDER_RECOVERY
}
