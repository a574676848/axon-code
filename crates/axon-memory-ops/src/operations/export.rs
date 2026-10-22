use std::{fmt::Write as _, fs, path::PathBuf};

use axon_crypto::sha256_tagged_hex;
use clap::Args;
use serde::Serialize;

use axon_agent_runtime::MemorySqliteRecordFilter;

use crate::{
    MEMORY_EXPORT_DIGEST_ALGORITHM, MEMORY_EXPORT_SCHEMA_VERSION, MemoryOpsOutputFormat,
    MemoryOpsResult, cli_error, json_result,
};

use super::{
    MemoryOpsContext, bounded_limit, display_path, now_ms,
    sync_audit::append_memory_export_audit,
    sync_encryption::{
        MEMORY_EXPORT_ENCRYPTION_STATUS_ENCRYPTED, MEMORY_EXPORT_ENCRYPTION_STATUS_PLAINTEXT,
        MemoryEncryptedExportEnvelope, encrypt_memory_export_payload,
    },
    sync_signature::{
        MEMORY_EXPORT_SIGNATURE_STATUS_ED25519, MEMORY_EXPORT_SIGNATURE_STATUS_UNSIGNED,
        MemoryExportSignatureView, sign_memory_export_digest,
    },
    view::{MemoryRecordView, record_view},
};

const MEMORY_EXPORT_STATUS_READY: &str = "memory_export_ready";
/// `axon memory export` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryExportCommand {
    /// 返回数量上限。
    #[arg(long)]
    pub limit: Option<usize>,
    /// 写入本机 JSON 文件；不指定时输出到 stdout。
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Ed25519 签名 key id；必须与 signing seed 一起使用。
    #[arg(long)]
    pub signing_key_id: Option<String>,
    /// Ed25519 32 字节签名种子十六进制；仅用于本次导出签名，不会写入导出包。
    #[arg(long)]
    pub signing_seed_hex: Option<String>,
    /// XChaCha20-Poly1305 加密 key id；必须与 encryption key 一起使用。
    #[arg(long)]
    pub encryption_key_id: Option<String>,
    /// XChaCha20-Poly1305 32 字节加密 key 十六进制；仅用于本次导出，不会写入导出包。
    #[arg(long)]
    pub encryption_key_hex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryExportReport {
    status: &'static str,
    schema_version: u32,
    generated_at_ms: u64,
    limit: usize,
    total_records: usize,
    record_count: usize,
    records_truncated: bool,
    output_path: Option<String>,
    payload_digest_algorithm: &'static str,
    payload_digest: String,
    signature_status: String,
    encryption_status: String,
    signature: Option<MemoryExportSignatureView>,
    records: Vec<MemoryRecordView>,
    memory_summary_exported: bool,
    prompt_text_exported: bool,
    provider_raw_payload_exported: bool,
    remote_write_allowed: bool,
}

/// 执行 Memory ref-only 导出。
#[must_use]
pub fn run_memory_export(
    format: MemoryOpsOutputFormat,
    context: &MemoryOpsContext,
    command: &MemoryExportCommand,
) -> MemoryOpsResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let store = match context.orchestrator.sqlite_store() {
        Ok(store) => store,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let summary = match store.summary() {
        Ok(summary) => summary,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let records = match store.records_limited(MemorySqliteRecordFilter::All, limit) {
        Ok(records) => records,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let records = records.iter().map(record_view).collect::<Vec<_>>();
    let payload_digest = match records_digest(&records) {
        Ok(digest) => digest,
        Err(error) => return cli_error(error),
    };
    let signature = match export_signature(&payload_digest, command) {
        Ok(signature) => signature,
        Err(error) => return cli_error(error),
    };
    let signature_status = if signature.is_some() {
        MEMORY_EXPORT_SIGNATURE_STATUS_ED25519
    } else {
        MEMORY_EXPORT_SIGNATURE_STATUS_UNSIGNED
    };
    let encryption_status = match export_encryption_status(command) {
        Ok(status) => status,
        Err(error) => return cli_error(error),
    };
    let report = MemoryExportReport {
        status: MEMORY_EXPORT_STATUS_READY,
        schema_version: MEMORY_EXPORT_SCHEMA_VERSION,
        generated_at_ms: now_ms(),
        limit,
        total_records: summary.total_records,
        record_count: records.len(),
        records_truncated: summary.total_records > records.len(),
        output_path: command.output.as_deref().map(display_path),
        payload_digest_algorithm: MEMORY_EXPORT_DIGEST_ALGORITHM,
        payload_digest,
        signature_status: signature_status.to_owned(),
        encryption_status: encryption_status.to_owned(),
        signature,
        records,
        memory_summary_exported: true,
        prompt_text_exported: false,
        provider_raw_payload_exported: false,
        remote_write_allowed: false,
    };
    if let Some(output) = &command.output
        && let Err(error) = write_export(output, &report, command)
    {
        return cli_error(error);
    }
    if let Err(error) = append_memory_export_audit(
        context,
        report.status,
        report.record_count,
        report.payload_digest_algorithm,
        report.payload_digest.clone(),
        report.signature_status.clone(),
        command.output.clone(),
    ) {
        return cli_error(error);
    }
    render_export(format, &report)
}

fn export_signature(
    payload_digest: &str,
    command: &MemoryExportCommand,
) -> Result<Option<MemoryExportSignatureView>, &'static str> {
    match (&command.signing_key_id, &command.signing_seed_hex) {
        (None, None) => Ok(None),
        (Some(key_id), Some(seed)) => {
            sign_memory_export_digest(payload_digest, key_id, seed).map(Some)
        }
        _ => Err("memory export 签名参数必须同时提供 signing-key-id 与 signing-seed-hex"),
    }
}

fn export_encryption_status(command: &MemoryExportCommand) -> Result<&'static str, String> {
    match (&command.encryption_key_id, &command.encryption_key_hex) {
        (None, None) => Ok(MEMORY_EXPORT_ENCRYPTION_STATUS_PLAINTEXT),
        (Some(_), Some(_)) => Ok(MEMORY_EXPORT_ENCRYPTION_STATUS_ENCRYPTED),
        _ => Err(
            "memory export 加密参数必须同时提供 encryption-key-id 与 encryption-key-hex".to_owned(),
        ),
    }
}

fn write_export(
    output: &PathBuf,
    report: &MemoryExportReport,
    command: &MemoryExportCommand,
) -> Result<(), String> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|_| "memory export 输出目录创建失败".to_owned())?;
    }
    let payload = serde_json::to_string_pretty(report)
        .map_err(|_| "memory export JSON 序列化失败".to_owned())?;
    let payload = if let (Some(key_id), Some(key_hex)) =
        (&command.encryption_key_id, &command.encryption_key_hex)
    {
        let envelope = encrypted_export_envelope(payload.as_bytes(), report, key_id, key_hex)?;
        serde_json::to_string_pretty(&envelope)
            .map_err(|_| "memory export 加密 envelope 序列化失败".to_owned())?
    } else {
        payload
    };
    fs::write(output, format!("{payload}\n")).map_err(|_| "memory export 文件写入失败".to_owned())
}

fn encrypted_export_envelope(
    payload: &[u8],
    report: &MemoryExportReport,
    key_id: &str,
    key_hex: &str,
) -> Result<MemoryEncryptedExportEnvelope, String> {
    encrypt_memory_export_payload(
        payload,
        &report.payload_digest,
        report.schema_version,
        key_id,
        key_hex,
    )
    .map_err(str::to_owned)
}

fn records_digest(records: &[MemoryRecordView]) -> Result<String, String> {
    let value = serde_json::to_value(records)
        .map_err(|_| "memory export digest payload 序列化失败".to_owned())?;
    let payload = serde_json::to_vec(&value)
        .map_err(|_| "memory export digest payload 序列化失败".to_owned())?;
    Ok(sha256_tagged_hex(&payload))
}

fn render_export(format: MemoryOpsOutputFormat, report: &MemoryExportReport) -> MemoryOpsResult {
    match format {
        MemoryOpsOutputFormat::Json => json_result(report),
        MemoryOpsOutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "Memory export: records={} output={} digest={}",
                report.record_count,
                report.output_path.as_deref().unwrap_or("stdout"),
                report.payload_digest
            );
            MemoryOpsResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}
