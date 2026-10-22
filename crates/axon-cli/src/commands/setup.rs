use super::*;
#[path = "setup/persistence.rs"]
mod bootstrap;
#[path = "setup/auth.rs"]
mod setup_auth;
#[path = "setup/interactive.rs"]
mod setup_interactive;
#[path = "setup/interactive_canvas.rs"]
mod setup_interactive_canvas;
#[path = "setup/interactive_config.rs"]
mod setup_interactive_config;
#[path = "setup/interactive_model_catalog.rs"]
mod setup_interactive_model_catalog;
#[path = "setup/interactive_prompts.rs"]
mod setup_interactive_prompts;
#[path = "setup/interactive_provider_catalog.rs"]
mod setup_interactive_provider_catalog;
#[path = "setup/interactive_provider_model.rs"]
mod setup_interactive_provider_model;
#[path = "setup/interactive_style.rs"]
mod setup_interactive_style;
#[path = "setup/interactive_views.rs"]
mod setup_interactive_views;
#[path = "setup/option_help.rs"]
mod setup_option_help;
#[path = "setup/plan.rs"]
mod setup_plan;
#[path = "setup/plan_entries.rs"]
mod setup_plan_entries;
#[cfg(test)]
#[path = "setup/plan_tests.rs"]
mod setup_plan_tests;
#[path = "setup/report.rs"]
mod setup_report;

use crate::management_commands::json_result;
use axon_config::AxonConfig;
use axon_config::setup::{render_env_from, secret_status};
use bootstrap::SetupTransaction;
use setup_interactive::collect_interactive_setup_input;
use setup_interactive_config::AgentSetupConfig;
use setup_interactive_views::confirm_setup_commit;
use setup_plan::build_setup_plan;
use setup_report::{
    SetupReportInput, build_setup_report, json_error_result, render_interactive_setup_report,
    setup_text,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) use setup_auth::run_auth;

const SETUP_ENV_FILE_NAME: &str = ".env";
const GATEWAY_API_TOKEN_BYTES: usize = 32;

pub(crate) fn run_setup(
    command: &SetupCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let out_format = command.format.unwrap_or(format);

    let path = match resolved_setup_config_path(config_path) {
        Ok(p) => p,
        Err(e) => return format_error_result(out_format, e, Vec::new()),
    };
    let env_path = env_path_for_config(&path);

    let agent_config = match load_agent_setup_config(command.agent_config.as_deref()) {
        Ok(cfg) => cfg,
        Err(e) => return format_error_result(out_format, &e, Vec::new()),
    };

    let mut transaction = match SetupTransaction::begin(&path, &env_path) {
        Ok(tx) => tx,
        Err(e) => return format_error_result(out_format, &e, Vec::new()),
    };

    let current_loaded = match ConfigLoader::load_from_path(&path) {
        Ok(l) => l,
        Err(e) => {
            transaction.cleanup();
            return format_error_result(out_format, &format!("{e:?}"), Vec::new());
        }
    };
    let mut generated_gateway_token: Option<(String, String)> = None;

    let (plan_result, cfg_bytes_opt, env_bytes_opt, interactive) = loop {
        let interactive_input =
            collect_interactive_setup_input(command, out_format, &current_loaded.config, &env_path);
        let interactive = interactive_input.interactive;

        if interactive_input.cancelled {
            transaction.cleanup();
            return CliRunResult {
                exit_code: 0,
                stdout: "Axon setup: 已取消操作 (零写入退出)\n".to_owned(),
                stderr: String::new(),
            };
        }

        let mut plan_result = match build_setup_plan(
            command,
            &agent_config,
            &interactive_input,
            &current_loaded.config,
        ) {
            Ok(p) => p,
            Err(e) => {
                transaction.cleanup();
                return format_error_result(out_format, &e, Vec::new());
            }
        };
        let gateway_token_env = plan_result
            .candidate_config
            .remote
            .gateway
            .api_token_env
            .clone();
        if generated_gateway_token.is_none()
            && secret_status(&env_path, &gateway_token_env)
                != axon_config::setup::SETUP_SECRET_PRESENT
        {
            let token = match generate_gateway_token() {
                Ok(token) => token,
                Err(error) => {
                    transaction.cleanup();
                    return format_error_result(out_format, &error, Vec::new());
                }
            };
            generated_gateway_token = Some((gateway_token_env.clone(), token));
        }
        if let Some((env_name, token)) = generated_gateway_token.as_ref() {
            plan_result
                .env_changes
                .push(axon_config::setup::EnvKeyChange::Set(
                    env_name.clone(),
                    token.clone(),
                ));
            plan_result
                .changed_summary
                .push("gateway_api_token.generated".to_owned());
        }

        let cfg_bytes_opt = if plan_result.candidate_config == current_loaded.config {
            None
        } else {
            match axon_config::to_toml_string(&plan_result.candidate_config) {
                Ok(text) => Some(text.into_bytes()),
                Err(e) => {
                    transaction.cleanup();
                    return format_error_result(out_format, &format!("{e:?}"), Vec::new());
                }
            }
        };

        let raw_env = fs::read(&env_path).ok();
        let env_bytes_opt = render_env_from(raw_env.as_deref(), &plan_result.env_changes);
        let has_changes = cfg_bytes_opt.is_some() || env_bytes_opt.is_some();

        if interactive_input.interactive
            && has_changes
            && !confirm_setup_commit(
                &path,
                &env_path,
                &plan_result.changed_summary,
                cfg_bytes_opt.is_some(),
                env_bytes_opt.is_some(),
            )
        {
            continue;
        }

        break (plan_result, cfg_bytes_opt, env_bytes_opt, interactive);
    };
    let has_changes = cfg_bytes_opt.is_some() || env_bytes_opt.is_some();
    let write_count = usize::from(cfg_bytes_opt.is_some()) + usize::from(env_bytes_opt.is_some());
    let candidate_digest = candidate_digest(cfg_bytes_opt.as_deref(), env_bytes_opt.as_deref());

    if has_changes {
        match transaction.prepare(cfg_bytes_opt.as_deref(), env_bytes_opt.as_deref()) {
            Ok(_) => {}
            Err(e) => {
                transaction.abort_prepare();
                return format_error_result(out_format, &e, Vec::new());
            }
        }
        match transaction.commit() {
            Ok(_) => {}
            Err(e) => {
                let file_states = transaction.rollback();
                return format_error_result(out_format, &e, file_states);
            }
        }
    } else {
        transaction.cleanup();
    }

    let report = build_setup_report(SetupReportInput {
        provider_name: &plan_result.effective_provider_name,
        path: &path,
        env_path: &env_path,
        config_changed: cfg_bytes_opt.is_some(),
        env_changed: env_bytes_opt.is_some(),
        config: &plan_result.candidate_config,
        model_id: &plan_result.effective_model_id,
        api_key_env: &plan_result.effective_api_key_env,
        gateway_api_token: generated_gateway_token
            .as_ref()
            .map(|(_, token)| token.clone()),
        changed: plan_result.changed_summary,
        validate: !(command.no_validate || agent_config.no_validate.unwrap_or(false)),
        mode: "local",
        base_revision: None,
        candidate_digest,
        write_count,
    });

    match out_format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: if interactive {
                render_interactive_setup_report(&report);
                String::new()
            } else {
                setup_text(&report)
            },
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(&report),
    }
}

fn candidate_digest(config: Option<&[u8]>, env: Option<&[u8]>) -> Option<String> {
    if config.is_none() && env.is_none() {
        return None;
    }
    let mut digest = Sha256::new();
    for (kind, bytes) in [(b"config\0".as_slice(), config), (b"env\0".as_slice(), env)] {
        digest.update(kind);
        if let Some(bytes) = bytes {
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        } else {
            digest.update(0u64.to_le_bytes());
        }
    }
    Some(format!("sha256:{}", hex::encode(digest.finalize())))
}

fn generate_gateway_token() -> Result<String, String> {
    let bytes = axon_crypto::random_bytes::<GATEWAY_API_TOKEN_BYTES>()
        .map_err(|error| format!("生成 Gateway API token 失败: {error}"))?;
    Ok(hex::encode(bytes))
}

fn format_error_result(
    format: OutputFormat,
    msg: &str,
    file_states: Vec<bootstrap::FileStateReport>,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_error_result(msg, file_states),
        OutputFormat::Text => CliRunResult {
            exit_code: 2,
            stdout: format!("Axon setup 错误: {msg}\n"),
            stderr: String::new(),
        },
    }
}

pub(crate) fn resolved_setup_config_path(
    config_path: Option<&Path>,
) -> Result<PathBuf, &'static str> {
    if let Some(path) = config_path {
        return Ok(path.to_path_buf());
    }
    ConfigLoader::default_user_config_path().map_err(|_| "无法解析用户配置路径")
}

pub(crate) fn env_path_for_config(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(SETUP_ENV_FILE_NAME)
}

fn load_agent_setup_config(path: Option<&Path>) -> Result<AgentSetupConfig, String> {
    let Some(path) = path else {
        return Ok(AgentSetupConfig::default());
    };
    let text = std::fs::read_to_string(path).map_err(|_| "无法读取 agent setup JSON".to_owned())?;
    let config: AgentSetupConfig =
        serde_json::from_str(&text).map_err(|_| "agent setup JSON 无效".to_owned())?;
    Ok(config)
}
