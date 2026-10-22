use super::*;
use crate::management_commands::{cli_config_error, cli_error, json_result};
use axon_display::display_path;

const SETUP_SECRET_NOT_REQUIRED: &str = "not_required";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatusReport {
    config_path: String,
    env_path: String,
    providers: Vec<AuthProviderStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthProviderStatus {
    name: String,
    protocol: String,
    base_url: String,
    credential_type: String,
    credential_status: &'static str,
    api_key_env: Option<String>,
    refresh_capable: bool,
    models: Vec<String>,
}

pub(crate) fn run_auth(
    command: &AuthCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_setup_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let subcommand =
        command
            .command
            .as_ref()
            .unwrap_or(&AuthSubcommand::Status(AuthStatusCommand {
                provider: None,
            }));
    match subcommand {
        AuthSubcommand::Status(status) => run_auth_status(
            command.format.unwrap_or(format),
            &path,
            status.provider.as_deref(),
        ),
    }
}

fn run_auth_status(
    format: OutputFormat,
    path: &Path,
    provider_filter: Option<&str>,
) -> CliRunResult {
    let loaded = match ConfigLoader::load_from_path(path) {
        Ok(loaded) => loaded,
        Err(error) => return cli_config_error(&error),
    };
    let env_path = env_path_for_config(path);
    let providers = loaded
        .config
        .providers
        .iter()
        .filter(|provider| provider_filter.is_none_or(|name| provider.name == name))
        .map(|provider| auth_provider_status(&loaded.config, provider, &env_path))
        .collect::<Vec<_>>();
    if providers.is_empty() && provider_filter.is_some() {
        return cli_error("provider 不存在");
    }
    let report = AuthStatusReport {
        config_path: display_path(path),
        env_path: display_path(&env_path),
        providers,
    };
    match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: auth_status_text(&report),
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(&report),
    }
}

fn auth_provider_status(
    config: &AxonConfig,
    provider: &ProviderConfig,
    env_path: &Path,
) -> AuthProviderStatus {
    let models = config
        .models
        .iter()
        .filter(|model| model.provider == provider.name)
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    let (credential_type, credential_status) = if provider
        .api_key
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        ("api_key".to_owned(), "present")
    } else if let Some(env) = provider.api_key_env.as_deref() {
        ("api_key_env".to_owned(), secret_status(env_path, env))
    } else {
        ("none".to_owned(), SETUP_SECRET_NOT_REQUIRED)
    };
    AuthProviderStatus {
        name: provider.name.clone(),
        protocol: provider.protocol.clone(),
        base_url: provider.base_url.clone(),
        credential_type,
        credential_status,
        api_key_env: provider.api_key_env.clone(),
        refresh_capable: false,
        models,
    }
}

fn auth_status_text(report: &AuthStatusReport) -> String {
    let mut output = format!("Axon auth status\n- config: {}\n", report.config_path);
    let _ = writeln!(output, "- env: {}", report.env_path);
    if report.providers.is_empty() {
        output.push_str("- providers: none\n");
        return output;
    }
    for provider in &report.providers {
        let _ = writeln!(
            output,
            "- {}: credential={} status={} refresh_capable={} models={}",
            provider.name,
            provider.credential_type,
            provider.credential_status,
            provider.refresh_capable,
            provider.models.join(",")
        );
    }
    output
}
