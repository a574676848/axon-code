use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{DOCTOR_CHECK_PROVIDER_RECOVERY, OutputFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DoctorStatus {
    Ok,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DoctorCheck {
    pub name: String,
    pub status: DoctorStatus,
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

const PROVIDER_RECOVERY_RECEIPT_SCHEMA_VERSION: u32 = 1;
const PROVIDER_RECOVERY_STATUS_OK: &str = "ok";
const PROVIDER_RECOVERY_PROTOCOL_OPENAI: &str = "openai";
const PROVIDER_RECOVERY_PROTOCOL_ANTHROPIC: &str = "anthropic";
const PROVIDER_RECOVERY_PROTOCOL_DASHSCOPE: &str = "dashscope";

/// doctor 回执模板写入报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DoctorReceiptTemplateReport {
    pub status: DoctorStatus,
    pub check: String,
    pub output: String,
    pub message: String,
}

pub(crate) fn write_receipt_template(
    check: Option<&str>,
    output: &Path,
) -> DoctorReceiptTemplateReport {
    let Some(check) = check else {
        return failed_template_report("", output, "生成回执模板需要 --check");
    };
    let Some(template) = receipt_template(check) else {
        return failed_template_report(check, output, "该 doctor 检查项没有回执模板");
    };
    if let Err(error) = fs::write(output, template) {
        return failed_template_report(check, output, format!("回执模板写入失败：{error}"));
    }
    DoctorReceiptTemplateReport {
        status: DoctorStatus::Ok,
        check: check.to_owned(),
        output: display_path(output),
        message: "回执模板已写入".to_owned(),
    }
}

pub(crate) fn render_template_report(
    report: &DoctorReceiptTemplateReport,
    format: OutputFormat,
) -> crate::CliRunResult {
    let exit_code = u8::from(report.status != DoctorStatus::Ok);
    let stdout = match format {
        OutputFormat::Json => format!(
            "{}\n",
            serde_json::to_string(report).expect("DoctorReceiptTemplateReport 序列化不应失败")
        ),
        OutputFormat::Text => format!(
            "Axon doctor receipt-template: {} - check={} output={} {}\n",
            report.status.as_str(),
            report.check,
            report.output,
            report.message
        ),
    };
    crate::CliRunResult {
        exit_code,
        stdout,
        stderr: String::new(),
    }
}

pub(crate) fn doctor_provider_recovery_check(receipt_path: Option<&Path>) -> DoctorCheck {
    let Some(receipt_path) = receipt_path else {
        return DoctorCheck::failed(
            DOCTOR_CHECK_PROVIDER_RECOVERY,
            "外部真实 provider 恢复验收需要 --provider-receipt 指向 JSON 回执",
        );
    };
    let Ok(receipt_text) = fs::read_to_string(receipt_path) else {
        return DoctorCheck::failed(DOCTOR_CHECK_PROVIDER_RECOVERY, "provider 恢复回执读取失败");
    };
    let Ok(receipt) = serde_json::from_str::<ProviderRecoveryReceipt>(&receipt_text) else {
        return DoctorCheck::failed(
            DOCTOR_CHECK_PROVIDER_RECOVERY,
            "provider 恢复回执 JSON 无效",
        );
    };

    if let Err(message) = validate_provider_recovery_receipt(&receipt) {
        return DoctorCheck::failed(DOCTOR_CHECK_PROVIDER_RECOVERY, message);
    }
    DoctorCheck::ok(
        DOCTOR_CHECK_PROVIDER_RECOVERY,
        format!(
            "外部真实 provider 恢复回执通过：provider={}，protocol={}，model={}，recovered_delta_count={}，recovered_content_chars={}",
            receipt.provider_name,
            receipt.protocol,
            receipt.model_id,
            receipt.recovered_delta_count,
            receipt.recovered_content_chars
        ),
    )
}

fn failed_template_report(
    check: &str,
    output: &Path,
    message: impl Into<String>,
) -> DoctorReceiptTemplateReport {
    DoctorReceiptTemplateReport {
        status: DoctorStatus::Failed,
        check: check.to_owned(),
        output: display_path(output),
        message: message.into(),
    }
}

fn receipt_template(check: &str) -> Option<&'static str> {
    match check {
        DOCTOR_CHECK_PROVIDER_RECOVERY => Some(PROVIDER_RECOVERY_TEMPLATE),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderRecoveryReceipt {
    schema_version: u32,
    check: String,
    recorded_at_ms: u64,
    operator: String,
    provider_name: String,
    protocol: String,
    model_id: String,
    live_provider: bool,
    simulated_provider: bool,
    stream_interrupted: bool,
    resume_retry_observed: bool,
    resume_marker_observed: bool,
    recovered_delta_count: usize,
    recovered_content_chars: usize,
    provider_raw_payload_allowed: bool,
    final_status: String,
}

fn validate_provider_recovery_receipt(
    receipt: &ProviderRecoveryReceipt,
) -> Result<(), &'static str> {
    if receipt.schema_version != PROVIDER_RECOVERY_RECEIPT_SCHEMA_VERSION
        || receipt.check != DOCTOR_CHECK_PROVIDER_RECOVERY
        || receipt.recorded_at_ms == 0
    {
        return Err("provider 恢复回执基础字段无效");
    }
    if receipt.operator.trim().is_empty()
        || receipt.provider_name.trim().is_empty()
        || receipt.model_id.trim().is_empty()
    {
        return Err("provider 恢复回执缺少操作者、provider 或模型信息");
    }
    if !matches!(
        receipt.protocol.as_str(),
        PROVIDER_RECOVERY_PROTOCOL_OPENAI
            | PROVIDER_RECOVERY_PROTOCOL_ANTHROPIC
            | PROVIDER_RECOVERY_PROTOCOL_DASHSCOPE
    ) {
        return Err("provider 恢复回执协议无效");
    }
    if !receipt.live_provider || receipt.simulated_provider {
        return Err("provider 恢复回执必须来自外部真实 provider");
    }
    if !receipt.stream_interrupted
        || !receipt.resume_retry_observed
        || !receipt.resume_marker_observed
        || receipt.recovered_delta_count == 0
        || receipt.recovered_content_chars == 0
    {
        return Err("provider 恢复回执缺少断流恢复证据");
    }
    if receipt.provider_raw_payload_allowed {
        return Err("provider 恢复回执要求 provider raw payload 关闭");
    }
    if receipt.final_status != PROVIDER_RECOVERY_STATUS_OK {
        return Err("provider 恢复回执最终状态未通过");
    }
    Ok(())
}

const PROVIDER_RECOVERY_TEMPLATE: &str = r#"{
  "schemaVersion": 1,
  "check": "provider-recovery",
  "recordedAtMs": 0,
  "operator": "",
  "providerName": "",
  "protocol": "openai",
  "modelId": "",
  "liveProvider": true,
  "simulatedProvider": false,
  "streamInterrupted": true,
  "resumeRetryObserved": true,
  "resumeMarkerObserved": true,
  "recoveredDeltaCount": 0,
  "recoveredContentChars": 0,
  "providerRawPayloadAllowed": false,
  "finalStatus": "ok"
}
"#;

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
