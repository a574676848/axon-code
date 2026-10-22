use super::*;
use crate::{run_commands::load_run_env_file, setup_commands::env_path_for_config};
use clap_complete::{Shell, generate};

#[path = "management/debug.rs"]
mod management_debug;
#[path = "management/debug_mailbox.rs"]
mod management_debug_mailbox;
#[path = "management/debug_realtime.rs"]
mod management_debug_realtime;

use crate::provider_report::{ProviderTestReport, ProviderTestStatus};
pub use management_debug::DebugReport;

pub(crate) fn run_models(
    command: &ModelsCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    match ConfigLoader::load_from_path(&path) {
        Ok(loaded) => {
            let models: Vec<&ModelConfig> = loaded
                .config
                .models
                .iter()
                .filter(|model| loaded.config.model_is_enabled(model))
                .filter(|model| {
                    command
                        .provider
                        .as_ref()
                        .is_none_or(|provider| &model.provider == provider)
                })
                .collect();
            match format {
                OutputFormat::Text => {
                    let mut stdout = String::new();
                    if models.is_empty() {
                        stdout.push_str("未配置模型\n");
                    } else {
                        for model in models {
                            let _ = writeln!(
                                stdout,
                                "{}\t{}\tctx={}\tvision={}",
                                model.id,
                                model.provider,
                                model.context_window,
                                model.supports_vision
                            );
                        }
                    }
                    CliRunResult {
                        exit_code: 0,
                        stdout,
                        stderr: String::new(),
                    }
                }
                OutputFormat::Json => json_result(&serde_json::json!({ "models": models })),
            }
        }
        Err(error) => cli_config_error(&error),
    }
}

pub(crate) fn run_providers(
    command: &ProvidersCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    match &command.command {
        None => run_providers_list(format, &path),
        Some(ProviderCommand::Add(command)) => run_provider_add(format, &path, command),
        Some(ProviderCommand::Remove(command)) => run_provider_remove(format, &path, &command.name),
        Some(ProviderCommand::Test(command)) => run_provider_test(format, &path, &command.name),
    }
}

fn run_providers_list(format: OutputFormat, path: &Path) -> CliRunResult {
    match ConfigLoader::load_from_path(path) {
        Ok(loaded) => match format {
            OutputFormat::Text => {
                let mut stdout = String::new();
                if loaded.config.providers.is_empty() {
                    stdout.push_str("未配置 provider\n");
                } else {
                    for provider in &loaded.config.providers {
                        let key = provider.api_key_env.as_deref().unwrap_or("-");
                        let websocket = provider.websocket_url.as_deref().unwrap_or("-");
                        let _ = writeln!(
                            stdout,
                            "{}\t{}\t{}\tapi_key_env={}\twire_transport={}\twebsocket_url={}",
                            provider.name,
                            provider.protocol,
                            provider.base_url,
                            key,
                            provider.wire_transport,
                            websocket
                        );
                    }
                }
                CliRunResult {
                    exit_code: 0,
                    stdout,
                    stderr: String::new(),
                }
            }
            OutputFormat::Json => json_result(&serde_json::json!({
                "providers": loaded.config.providers,
            })),
        },
        Err(error) => cli_config_error(&error),
    }
}

fn run_provider_add(
    format: OutputFormat,
    path: &Path,
    command: &ProviderAddCommand,
) -> CliRunResult {
    match ConfigLoader::load_from_path(path) {
        Ok(mut loaded) => {
            let mut provider = ProviderConfig::new(
                command.name.clone(),
                command.protocol.as_config_value(),
                command.base_url.clone(),
            );
            provider.api_key_env.clone_from(&command.api_key_env);
            command
                .wire_transport
                .as_config_value()
                .clone_into(&mut provider.wire_transport);
            provider.websocket_url.clone_from(&command.websocket_url);
            match add_provider(&mut loaded.config, provider.clone()) {
                Ok(()) => match ConfigLoader::save_to_path(path, &loaded.config) {
                    Ok(()) => match format {
                        OutputFormat::Text => CliRunResult {
                            exit_code: 0,
                            stdout: format!("已添加 provider: {}\n", provider.name),
                            stderr: String::new(),
                        },
                        OutputFormat::Json => json_result(&serde_json::json!({
                            "provider": provider,
                            "path": path.display().to_string(),
                        })),
                    },
                    Err(error) => cli_config_error(&error),
                },
                Err(error) => cli_config_error(&error),
            }
        }
        Err(error) => cli_config_error(&error),
    }
}

fn run_provider_remove(format: OutputFormat, path: &Path, name: &str) -> CliRunResult {
    match ConfigLoader::load_from_path(path) {
        Ok(mut loaded) => match remove_provider(&mut loaded.config, name) {
            Ok(()) => match ConfigLoader::save_to_path(path, &loaded.config) {
                Ok(()) => match format {
                    OutputFormat::Text => CliRunResult {
                        exit_code: 0,
                        stdout: format!("已移除 provider: {name}\n"),
                        stderr: String::new(),
                    },
                    OutputFormat::Json => json_result(&serde_json::json!({
                        "removed": name,
                        "path": path.display().to_string(),
                    })),
                },
                Err(error) => cli_config_error(&error),
            },
            Err(error) => cli_config_error(&error),
        },
        Err(error) => cli_config_error(&error),
    }
}

fn run_provider_test(format: OutputFormat, path: &Path, name: &str) -> CliRunResult {
    if let Err(error) = load_run_env_file(&env_path_for_config(path)) {
        return cli_error(error);
    }
    match ConfigLoader::load_from_path(path) {
        Ok(loaded) => match loaded
            .config
            .providers
            .iter()
            .find(|provider| provider.name == name)
        {
            Some(provider) => {
                let report = ProviderTestReport::run(provider);
                let exit_code = u8::from(report.status != ProviderTestStatus::Ok);
                match format {
                    OutputFormat::Text => CliRunResult {
                        exit_code,
                        stdout: report.to_text(),
                        stderr: String::new(),
                    },
                    OutputFormat::Json => {
                        let mut result = json_result(&report);
                        result.exit_code = exit_code;
                        result
                    }
                }
            }
            None => cli_error("provider 不存在"),
        },
        Err(error) => cli_config_error(&error),
    }
}

pub(crate) fn run_config(
    command: &ConfigCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let subcommand = command.command.as_ref().unwrap_or(&ConfigSubcommand::Show);

    match subcommand {
        ConfigSubcommand::Show => run_config_show(format, &path),
        ConfigSubcommand::Edit => run_config_edit(&path),
        ConfigSubcommand::Set(command) => run_config_set(format, &path, command),
        ConfigSubcommand::Get(command) => run_config_get(format, &path, &command.key),
        ConfigSubcommand::Validate => run_config_validate(format, &path),
        ConfigSubcommand::Keys => run_config_keys(format),
        ConfigSubcommand::Path => run_config_path(format, &path),
        ConfigSubcommand::EnvPath => run_config_env_path(format, &path),
    }
}

fn run_config_show(format: OutputFormat, path: &Path) -> CliRunResult {
    match ConfigLoader::load_from_path(path) {
        Ok(loaded) => match format {
            OutputFormat::Text => match to_toml_string(&loaded.config) {
                Ok(stdout) => CliRunResult {
                    exit_code: 0,
                    stdout,
                    stderr: String::new(),
                },
                Err(_) => cli_error("配置序列化失败"),
            },
            OutputFormat::Json => json_result(&loaded.config),
        },
        Err(error) => cli_config_error(&error),
    }
}

fn run_config_validate(format: OutputFormat, path: &Path) -> CliRunResult {
    match ConfigLoader::load_from_path(path) {
        Ok(_) => match format {
            OutputFormat::Text => CliRunResult {
                exit_code: 0,
                stdout: format!("Axon config: ok\n- path: {}\n", path.display()),
                stderr: String::new(),
            },
            OutputFormat::Json => json_result(&serde_json::json!({
                "status": "ok",
                "path": path.display().to_string(),
            })),
        },
        Err(error) => cli_config_error(&error),
    }
}

fn run_config_keys(format: OutputFormat) -> CliRunResult {
    match format {
        OutputFormat::Text => {
            let mut stdout = String::new();
            for spec in config_key_specs() {
                let editable = if spec.editable {
                    "editable"
                } else {
                    "readonly"
                };
                let _ = writeln!(
                    stdout,
                    "{}\t{:?}\t{}\t{}",
                    spec.key, spec.kind, editable, spec.description
                );
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
        OutputFormat::Json => json_result(&config_key_specs()),
    }
}

fn run_config_path(format: OutputFormat, path: &Path) -> CliRunResult {
    match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!("{}\n", path.display()),
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(&serde_json::json!({
            "path": path.display().to_string(),
        })),
    }
}

fn run_config_env_path(format: OutputFormat, path: &Path) -> CliRunResult {
    let env_path = env_path_for_config(path);
    match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!("{}\n", env_path.display()),
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(&serde_json::json!({
            "envPath": env_path.display().to_string(),
        })),
    }
}

fn run_config_get(format: OutputFormat, path: &Path, key: &str) -> CliRunResult {
    match ConfigLoader::load_from_path(path) {
        Ok(loaded) => match get_config_value(&loaded.config, key) {
            Some(value) => match format {
                OutputFormat::Text => CliRunResult {
                    exit_code: 0,
                    stdout: format!("{key} = {value}\n"),
                    stderr: String::new(),
                },
                OutputFormat::Json => json_result(&serde_json::json!({
                    "key": key,
                    "value": value,
                })),
            },
            None => cli_error("未知配置键"),
        },
        Err(error) => cli_config_error(&error),
    }
}

fn run_config_set(format: OutputFormat, path: &Path, command: &ConfigSetCommand) -> CliRunResult {
    match ConfigLoader::load_from_path(path) {
        Ok(mut loaded) => {
            match set_config_value(&mut loaded.config, &command.key, &command.value) {
                Ok(value) => match ConfigLoader::save_to_path(path, &loaded.config) {
                    Ok(()) => match format {
                        OutputFormat::Text => CliRunResult {
                            exit_code: 0,
                            stdout: format!("已写入配置: {} = {}\n", command.key, value),
                            stderr: String::new(),
                        },
                        OutputFormat::Json => json_result(&serde_json::json!({
                            "key": command.key,
                            "value": value,
                            "path": path.display().to_string(),
                        })),
                    },
                    Err(error) => cli_config_error(&error),
                },
                Err(error) => cli_config_error(&error),
            }
        }
        Err(error) => cli_config_error(&error),
    }
}

fn run_config_edit(path: &Path) -> CliRunResult {
    let loaded = match ConfigLoader::load_from_path(path) {
        Ok(loaded) => loaded,
        Err(error) => return cli_config_error(&error),
    };
    if let Err(error) = ConfigLoader::save_to_path(path, &loaded.config) {
        return cli_config_error(&error);
    }

    let editor = std::env::var("AXON_EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| default_editor());
    match Command::new(&editor).arg(path).status() {
        Ok(status) if status.success() => CliRunResult {
            exit_code: 0,
            stdout: format!("已编辑配置: {}\n", path.display()),
            stderr: String::new(),
        },
        Ok(_) => cli_error("配置编辑器返回失败状态"),
        Err(_) => cli_error("无法启动配置编辑器"),
    }
}

pub(crate) fn resolved_config_path(config_path: Option<&Path>) -> Result<PathBuf, &'static str> {
    if let Some(path) = config_path {
        return Ok(path.to_path_buf());
    }
    ConfigLoader::default_user_config_path().map_err(|_| "无法解析用户配置路径")
}

pub(crate) fn cli_config_error(error: &axon_domain::AxonError) -> CliRunResult {
    cli_error(format!("{error:?}"))
}

fn default_editor() -> String {
    if cfg!(windows) {
        "notepad".to_owned()
    } else {
        "vi".to_owned()
    }
}

pub(crate) fn cli_error(message: impl Into<String>) -> CliRunResult {
    CliRunResult {
        exit_code: 2,
        stdout: String::new(),
        stderr: format!("error: {}\n", message.into()),
    }
}

pub(crate) fn json_result(value: &impl Serialize) -> CliRunResult {
    match serde_json::to_string(value) {
        Ok(line) => CliRunResult {
            exit_code: 0,
            stdout: format!("{line}\n"),
            stderr: String::new(),
        },
        Err(_) => cli_error("JSON 序列化失败"),
    }
}

pub(crate) fn run_completion(command: &CompletionCommand) -> CliRunResult {
    let mut clap_command = AxonCli::command();
    let mut output = Vec::new();
    let shell: Shell = command.shell.into();
    generate(shell, &mut clap_command, "axon", &mut output);

    let stdout = match String::from_utf8(output) {
        Ok(stdout) => stdout,
        Err(error) => return cli_error(format!("clap completion 输出不是 UTF-8: {error}")),
    };

    CliRunResult {
        exit_code: 0,
        stdout,
        stderr: String::new(),
    }
}

pub(crate) fn run_debug(
    command: &DebugCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
    cwd: Option<&Path>,
) -> CliRunResult {
    management_debug::run_debug(command, format, config_path, cwd)
}

impl From<CompletionShell> for Shell {
    fn from(shell: CompletionShell) -> Self {
        match shell {
            CompletionShell::PowerShell => Self::PowerShell,
            CompletionShell::Bash => Self::Bash,
            CompletionShell::Zsh => Self::Zsh,
            CompletionShell::Fish => Self::Fish,
        }
    }
}

#[cfg(test)]
mod tests {
    use axon_config::{AxonConfig, ConfigLoader, ModelConfig, ProviderConfig};

    use super::*;

    #[test]
    fn models_command_excludes_disabled_provider_models() {
        let root = axon_test_support::test_temp_dir();
        let config_path = root.path().join("config.toml");
        let mut enabled =
            ProviderConfig::new("enabled-provider", "openai", "https://enabled.invalid/v1");
        let mut disabled =
            ProviderConfig::new("disabled-provider", "openai", "https://disabled.invalid/v1");
        enabled.enabled = true;
        disabled.enabled = false;
        let mut config = AxonConfig::default();
        config.providers.extend([enabled, disabled]);
        config.models.extend([
            model("enabled-model", "enabled-provider"),
            model("disabled-model", "disabled-provider"),
        ]);
        config.agent.default_model = "enabled-model".to_owned();
        config.agent.fast_model = "enabled-model".to_owned();
        ConfigLoader::save_to_path(&config_path, &config).unwrap();

        let result = run_models(
            &ModelsCommand {
                provider: None,
                format: None,
            },
            OutputFormat::Text,
            Some(&config_path),
        );

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("enabled-model"));
        assert!(!result.stdout.contains("disabled-model"));
    }

    fn model(id: &str, provider: &str) -> ModelConfig {
        ModelConfig {
            name: id.to_owned(),
            id: id.to_owned(),
            provider: provider.to_owned(),
            context_window: 32_000,
            ..ModelConfig::default()
        }
    }
}
