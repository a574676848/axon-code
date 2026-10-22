use std::path::Path;

use axon_agent_runtime::redact_agent_context_text;
use axon_config::AxonConfig;
use axon_config::provider_credential;
use axon_tui_runtime::{
    AgentTuiConfigDiagnostic, AgentTuiConfigDiagnosticStatus, AgentTuiConfigStatus,
};

use crate::{
    AxonCli,
    management_commands::resolved_config_path,
    provider_report::{ProviderTestReport, ProviderTestStatus},
};

use super::super::load_run_env_file;

const CONFIG_PATH_SCOPE: &str = "config_path";
const ENV_FILE_SCOPE: &str = "env_file";
const CONFIG_FILE_SCOPE: &str = "config_file";

pub(super) fn resolve_agent_tui_config_status(cli: &AxonCli) -> AgentTuiConfigStatus {
    let path = match resolved_config_path(cli.global.config.as_deref()) {
        Ok(path) => path,
        Err(error) => {
            let reason = log_config_runtime_error(CONFIG_PATH_SCOPE, None, error);
            return AgentTuiConfigStatus::warning(
                "配置路径解析失败",
                format!("{reason}；请检查 --config 参数或运行 axon setup"),
            );
        }
    };
    let env_path = crate::setup_commands::env_path_for_config(&path);
    let config_path_text = display_path(&path);
    let env_path_text = display_path(&env_path);
    if let Err(error) = load_run_env_file(&env_path) {
        let reason = log_config_runtime_error(ENV_FILE_SCOPE, Some(&env_path), &error);
        return AgentTuiConfigStatus::warning(
            "密钥文件加载失败",
            format!("{reason}；请检查文件权限或运行 axon setup"),
        )
        .with_paths(config_path_text, env_path_text);
    }
    let loaded = match axon_config::ConfigLoader::load_from_path(&path) {
        Ok(loaded) => loaded,
        Err(error) => {
            let error = format!("{error:?}");
            let reason = log_config_runtime_error(CONFIG_FILE_SCOPE, Some(&path), &error);
            return AgentTuiConfigStatus::warning(
                "配置文件加载失败",
                format!("{reason}；请修复配置或运行 axon setup"),
            )
            .with_paths(config_path_text, env_path_text);
        }
    };
    agent_tui_config_status_from_config(&loaded.config)
        .with_paths(config_path_text, env_path_text)
        .with_diagnostics(config_diagnostics(&loaded.config))
}

fn log_config_runtime_error(scope: &str, path: Option<&Path>, reason: &str) -> String {
    let reason = redact_agent_context_text(reason);
    let path = path.map_or_else(|| "none".to_owned(), display_path);
    axon_logger::error(&format!(
        "Agent TUI 配置运行时错误: scope={scope} path={path} reason={reason}"
    ));
    reason
}

fn agent_tui_config_status_from_config(config: &AxonConfig) -> AgentTuiConfigStatus {
    let Some(model) = default_status_model(config) else {
        return AgentTuiConfigStatus::warning("未配置", "未配置模型，请运行 axon setup");
    };
    let provider = config.enabled_provider(&model.provider);
    let provider_name = provider.map_or(model.provider.as_str(), |item| item.name.as_str());
    let api_key_status = provider.map_or_else(
        || "需要认证，运行 axon setup".to_owned(),
        |item| {
            if item
                .api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                return "已认证".to_owned();
            }
            match item.api_key_env.as_deref().and_then(provider_credential) {
                Some(_) => "已认证".to_owned(),
                None => "需要认证，运行 axon setup".to_owned(),
            }
        },
    );
    AgentTuiConfigStatus::new(provider_name, model.name.as_str(), api_key_status)
}

fn config_diagnostics(config: &AxonConfig) -> Vec<AgentTuiConfigDiagnostic> {
    let Some(model) = default_status_model(config) else {
        return vec![AgentTuiConfigDiagnostic::new(
            "model",
            AgentTuiConfigDiagnosticStatus::Failed,
            "未配置默认模型",
            Some("运行 axon setup 或 axon config set agent.default_model <model>".to_owned()),
        )];
    };
    let Some(provider) = config.enabled_provider(&model.provider) else {
        return vec![AgentTuiConfigDiagnostic::new(
            "provider",
            AgentTuiConfigDiagnosticStatus::Failed,
            format!(
                "模型 {} 绑定的 Provider {} 不存在或已禁用",
                model.name, model.provider
            ),
            Some("运行 axon setup 或 axon providers add 修复 provider 配置".to_owned()),
        )];
    };
    let report = ProviderTestReport::run(provider);
    vec![provider_diagnostic(&report)]
}

fn provider_diagnostic(report: &ProviderTestReport) -> AgentTuiConfigDiagnostic {
    let status = if report.status == ProviderTestStatus::Ok {
        AgentTuiConfigDiagnosticStatus::Ok
    } else {
        AgentTuiConfigDiagnosticStatus::Failed
    };
    let http_status = report
        .http_status
        .map_or(String::new(), |status| format!(" · HTTP {status}"));
    AgentTuiConfigDiagnostic::new(
        format!("provider.{}", report.name),
        status,
        format!(
            "{} · credential={}{} · {}",
            report.base_url,
            report.credential.as_str(),
            http_status,
            report.message
        ),
        provider_remediation(report),
    )
}

fn provider_remediation(report: &ProviderTestReport) -> Option<String> {
    if report.status == ProviderTestStatus::Ok {
        return None;
    }
    Some(match report.credential.as_str() {
        "missing" => {
            "检查密钥 env 文件后运行 axon setup，或在当前 shell 设置对应 API key".to_owned()
        }
        _ => "检查 provider base_url、网络连通性和模型服务状态；可运行 axon providers test <name>"
            .to_owned(),
    })
}

fn default_status_model(config: &AxonConfig) -> Option<&axon_config::ModelConfig> {
    let default_model = config.agent.default_model.as_str();
    config
        .models
        .iter()
        .filter(|model| config.model_is_enabled(model))
        .find(|model| model.name == default_model)
        .or_else(|| {
            config
                .models
                .iter()
                .filter(|model| config.model_is_enabled(model))
                .find(|model| model.id == default_model)
        })
        .or_else(|| {
            config
                .models
                .iter()
                .find(|model| config.model_is_enabled(model))
        })
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}
