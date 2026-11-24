use std::{fmt::Write as _, path::Path};

use axon_config::AxonConfig;
use axon_display::display_path;
use serde::Serialize;

use super::bootstrap::FileStateReport;
use super::setup_interactive::{model_summary, provider_protocol_summary};
use super::setup_interactive_canvas::render_setup_frame;
use super::setup_interactive_style::{SetupStyle, paint};
use crate::CliRunResult;
use axon_config::setup::secret_status;

const SETUP_STATUS_OK: &str = "ok";
const SETUP_STATUS_NEEDS_CREDENTIAL: &str = "needs_credential";
const SETUP_SECRET_PRESENT: &str = "present";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetupReport {
    status: &'static str,
    provider_name: String,
    model_id: String,
    config_path: String,
    env_path: String,
    config_changed: bool,
    env_changed: bool,
    changed: Vec<String>,
    secrets: Vec<SecretReport>,
    credential_status: &'static str,
    validation_status: &'static str,
    validation_messages: Vec<String>,
    mode: &'static str,
    #[serde(rename = "baseRevision", skip_serializing_if = "Option::is_none")]
    base_revision: Option<String>,
    #[serde(rename = "candidateDigest", skip_serializing_if = "Option::is_none")]
    candidate_digest: Option<String>,
    #[serde(rename = "writeCount")]
    write_count: usize,
    #[serde(skip_serializing)]
    gateway_api_token: Option<String>,
    next_commands: Vec<String>,
    summary: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretReport {
    env: String,
    status: &'static str,
}

pub(super) struct SetupReportInput<'a> {
    pub provider_name: &'a str,
    pub path: &'a Path,
    pub env_path: &'a Path,
    pub config_changed: bool,
    pub env_changed: bool,
    pub config: &'a AxonConfig,
    pub model_id: &'a str,
    pub api_key_env: &'a str,
    pub gateway_api_token: Option<String>,
    pub changed: Vec<String>,
    pub validate: bool,
    pub mode: &'static str,
    pub base_revision: Option<String>,
    pub candidate_digest: Option<String>,
    pub write_count: usize,
}

pub(super) fn build_setup_report(input: SetupReportInput<'_>) -> SetupReport {
    let credential_status = secret_status(input.env_path, input.api_key_env);
    let validation_messages = validation_messages(&input, credential_status);
    let summary = setup_summary(&input);
    let validation_status = if credential_status == SETUP_SECRET_PRESENT {
        SETUP_STATUS_OK
    } else {
        SETUP_STATUS_NEEDS_CREDENTIAL
    };
    SetupReport {
        status: validation_status,
        provider_name: input.provider_name.to_owned(),
        model_id: input.model_id.to_owned(),
        config_path: display_path(input.path),
        env_path: display_path(input.env_path),
        config_changed: input.config_changed,
        env_changed: input.env_changed,
        changed: input.changed,
        secrets: vec![SecretReport {
            env: input.api_key_env.to_owned(),
            status: credential_status,
        }],
        credential_status,
        validation_status,
        validation_messages,
        mode: input.mode,
        base_revision: input.base_revision,
        candidate_digest: input.candidate_digest,
        write_count: input.write_count,
        gateway_api_token: input.gateway_api_token,
        next_commands: next_commands(input.model_id),
        summary,
    }
}

fn validation_messages(
    input: &SetupReportInput<'_>,
    credential_status: &'static str,
) -> Vec<String> {
    if !input.validate {
        return Vec::new();
    }
    let mut messages = Vec::new();
    if input
        .config
        .providers
        .iter()
        .any(|provider| provider.name == input.provider_name)
    {
        messages.push("provider 已写入".to_owned());
    }
    if input
        .config
        .models
        .iter()
        .any(|model| model.provider == input.provider_name && model.id == input.model_id)
    {
        messages.push("model 已写入".to_owned());
    }
    if credential_status == SETUP_SECRET_PRESENT {
        messages.push("API key 已就绪".to_owned());
    } else {
        messages.push("API key 缺失，需补充后才能调用模型".to_owned());
    }
    messages
}

fn next_commands(model_id: &str) -> Vec<String> {
    vec![
        "axon".to_owned(),
        "axon auth status".to_owned(),
        "axon doctor --check provider".to_owned(),
        format!("axon run --model {model_id} \"你好\""),
    ]
}

fn setup_summary(input: &SetupReportInput<'_>) -> Vec<String> {
    vec![
        format!("provider: {}", input.provider_name),
        format!("model: {}", input.model_id),
        format!(
            "protocol: {}",
            provider_protocol_summary(input.config, input.provider_name)
        ),
        format!(
            "model: {}",
            model_summary(input.config, input.provider_name, input.model_id)
        ),
        format!("config: {}", input.path.display()),
        format!("env: {}", input.env_path.display()),
    ]
}

pub(super) fn render_interactive_setup_report(report: &SetupReport) {
    let (status_style, status_text) = match report.status {
        SETUP_STATUS_OK => (SetupStyle::Success, "配置已成功原子更新"),
        _ => (SetupStyle::Warning, "配置已更新，但 API Key 尚待配置"),
    };
    let config_status = if report.config_changed {
        paint(SetupStyle::Success, "已更新")
    } else {
        paint(SetupStyle::Muted, "未变更")
    };
    let env_status = if report.env_changed {
        paint(SetupStyle::Success, "已更新")
    } else {
        paint(SetupStyle::Muted, "未变更")
    };
    let mut lines = vec![
        paint(status_style, status_text),
        String::new(),
        format!("配置文件  {config_status}  {}", report.config_path),
        format!("环境文件  {env_status}  {}", report.env_path),
    ];
    if !report.changed.is_empty() {
        lines.extend([String::new(), paint(SetupStyle::Accent, "已应用变更")]);
        lines.extend(report.changed.iter().map(|item| format!("  + {item}")));
    }
    if !report.validation_messages.is_empty() {
        lines.extend([String::new(), paint(SetupStyle::Accent, "验证结果")]);
        lines.extend(
            report
                .validation_messages
                .iter()
                .map(|item| paint(SetupStyle::Muted, format!("  {item}"))),
        );
    }
    if let Some(token) = report.gateway_api_token.as_deref() {
        lines.extend([
            String::new(),
            paint(
                SetupStyle::Accent,
                "Gateway API token（仅本次生成，已写入 .env）",
            ),
            paint(SetupStyle::Success, format!("  {token}")),
        ]);
    }
    lines.extend([String::new(), paint(SetupStyle::Accent, "下一步")]);
    lines.extend(
        report
            .next_commands
            .iter()
            .enumerate()
            .map(|(index, command)| {
                paint(SetupStyle::Muted, format!("  {}. {command}", index + 1))
            }),
    );
    render_setup_frame(&paint(SetupStyle::Accent, "SETUP COMPLETE"), &lines);
}

pub(super) fn setup_text(report: &SetupReport) -> String {
    let (status_symbol, status_desc) = match report.status {
        SETUP_STATUS_OK => ("✔ OK", "配置已成功原子更新"),
        _ => ("⚠️ NEEDS_CREDENTIAL", "配置已更新，但 API Key 尚待配置"),
    };

    let mut output = String::new();
    let _ = writeln!(
        output,
        "┌─ 📝 CONFIG TRANSACTION REPORT ──────────────────────────────────────────────┐"
    );
    let _ = writeln!(
        output,
        "│ 状态: [{:<18}] {:<40} │",
        status_symbol, status_desc
    );
    let _ = writeln!(
        output,
        "├─────────────────────────────────────────────────────────────────────────────┤"
    );
    let _ = writeln!(
        output,
        "│ 📋 变更文件明细 (File Diff & Transaction):                                  │"
    );
    let config_tag = if report.config_changed {
        "UPDATED"
    } else {
        "UNTOUCHED"
    };
    let env_tag = if report.env_changed {
        "UPDATED"
    } else {
        "UNTOUCHED"
    };
    let _ = writeln!(output, "│  ├─ [{}] 📄 {}", config_tag, report.config_path);
    for item in &report.changed {
        let _ = writeln!(output, "│  │       + {}", item);
    }
    let _ = writeln!(output, "│  └─ [{}] {}", env_tag, report.env_path);
    let _ = writeln!(
        output,
        "├─────────────────────────────────────────────────────────────────────────────┤"
    );
    if let Some(token) = report.gateway_api_token.as_deref() {
        let _ = writeln!(output, "│ Gateway API token（仅本次生成）: {token}");
        let _ = writeln!(
            output,
            "├─────────────────────────────────────────────────────────────────────────────┤"
        );
    }
    let _ = writeln!(
        output,
        "│ 💡 下一步验证建议 (Next Steps):                                             │"
    );
    for (idx, cmd) in report.next_commands.iter().enumerate() {
        let _ = writeln!(output, "│  {}. {}", idx + 1, cmd);
    }
    let _ = writeln!(
        output,
        "└─────────────────────────────────────────────────────────────────────────────┘"
    );
    output
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonErrorOutput<'a> {
    outcome: &'static str,
    message: &'a str,
    file_states: Vec<FileStateReport>,
}

pub(super) fn json_error_result(message: &str, file_states: Vec<FileStateReport>) -> CliRunResult {
    let err_obj = JsonErrorOutput {
        outcome: "error",
        message,
        file_states,
    };
    let json_text = serde_json::to_string_pretty(&err_obj).unwrap_or_else(|_| "{}".to_owned());
    CliRunResult {
        exit_code: 2,
        stdout: json_text,
        stderr: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_report_omits_credential_environment_metadata() {
        let report = SetupReport {
            status: SETUP_STATUS_OK,
            provider_name: "provider".to_owned(),
            model_id: "model".to_owned(),
            config_path: "config.toml".to_owned(),
            env_path: ".env".to_owned(),
            config_changed: true,
            env_changed: false,
            changed: vec!["configValue.agent.fast_model".to_owned()],
            secrets: vec![SecretReport {
                env: "AXON_DENG_API_KEY".to_owned(),
                status: SETUP_SECRET_PRESENT,
            }],
            credential_status: SETUP_SECRET_PRESENT,
            validation_status: SETUP_STATUS_OK,
            validation_messages: Vec::new(),
            mode: "bootstrap",
            base_revision: None,
            candidate_digest: None,
            write_count: 1,
            gateway_api_token: None,
            next_commands: Vec::new(),
            summary: Vec::new(),
        };

        let output = setup_text(&report);

        assert!(!output.contains("环境变量"));
        assert!(!output.contains("AXON_DENG_API_KEY"));
    }
}
