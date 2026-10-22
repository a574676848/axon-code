//! 配置状态检测与 setup 引导。

use std::path::Path;

use axon_config::ConfigLoader;

use crate::{
    AxonCli, AxonCommand, CliRunResult, OutputFormat, setup_commands::resolved_setup_config_path,
};

/// 检查配置是否已初始化（存在配置文件且至少有一个 provider）。
pub(crate) fn is_configured(config_path: Option<&Path>) -> bool {
    let Ok(path) = resolved_setup_config_path(config_path) else {
        return false;
    };

    if !path.exists() {
        return false;
    }

    let Ok(loaded) = ConfigLoader::load_from_path(&path) else {
        return false;
    };

    !loaded.config.providers.is_empty()
}

/// 判断命令是否需要配置检查。
fn command_needs_config(command: Option<&AxonCommand>) -> bool {
    !matches!(
        command,
        Some(
            AxonCommand::Setup(_)
                | AxonCommand::Auth(_)
                | AxonCommand::Config(_)
                | AxonCommand::Models(_)
                | AxonCommand::Providers(_)
                | AxonCommand::Completion(_)
                | AxonCommand::Upgrade(_)
                | AxonCommand::Doctor(_)
                | AxonCommand::Usage(_)
        )
    )
}

/// 检查配置状态，如果未配置则返回引导信息。
pub(crate) fn check_and_guide_setup(cli: &AxonCli) -> Option<CliRunResult> {
    if cfg!(test) {
        return None;
    }

    // 不需要配置检查的命令直接跳过
    if !command_needs_config(cli.command.as_ref()) {
        return None;
    }

    // 配置已存在，跳过
    if is_configured(cli.global.config.as_deref()) {
        return None;
    }

    // 配置不存在，返回引导信息
    Some(guide_to_setup(cli.global.format))
}

/// 生成 setup 引导信息。
fn guide_to_setup(format: OutputFormat) -> CliRunResult {
    match format {
        OutputFormat::Text => {
            let message = r#"
🚀 欢迎使用 Axon！

检测到您是第一次使用 Axon，需要先完成初始化配置。

请运行以下命令完成配置：

    axon setup

该命令将引导您：
  • 配置模型提供商
  • 设置 API key 环境变量
  • 选择默认模型

配置完成后，您就可以：
  • 输入 `axon` 进入交互式主对话
  • 使用 `axon run "<prompt>"` 快速执行 AI 任务
  • 探索更多功能 `axon --help`

需要帮助？访问文档：https://docs.axon.dev
"#;
            CliRunResult {
                exit_code: 0,
                stdout: message.to_owned(),
                stderr: String::new(),
            }
        }
        OutputFormat::Json => {
            let json = serde_json::json!({
                "status": "needs_setup",
                "message": "Axon 尚未配置，请先运行 `axon setup` 完成初始化",
                "next_command": "axon setup",
                "documentation": "https://docs.axon.dev"
            });
            let stdout = match serde_json::to_string_pretty(&json) {
                Ok(stdout) => stdout,
                Err(error) => {
                    return CliRunResult {
                        exit_code: 1,
                        stdout: String::new(),
                        stderr: format!("setup 引导 JSON 序列化失败: {error}\n"),
                    };
                }
            };
            CliRunResult {
                exit_code: 0,
                stdout: format!("{stdout}\n"),
                stderr: String::new(),
            }
        }
    }
}
