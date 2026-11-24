use std::{fmt::Write as _, fs, path::PathBuf};

use axon_agent_runtime::{MemorySqliteRecord, MemoryTextDocument};
use axon_crypto::sha256_tagged_hex;
use clap::Args;
use serde::Deserialize;

use crate::{
    MEMORY_EXPORT_DIGEST_ALGORITHM, MEMORY_EXPORT_SCHEMA_VERSION, MemoryOpsOutputFormat,
    MemoryOpsResult, cli_error, json_result,
};

#[path = "import_record.rs"]
mod import_record;

use self::import_record::{
    ImportRecordRef, import_record_refs, memory_state_code, sqlite_record_from_import,
};
use super::{
    MemoryOpsContext,
    sync_audit::{MemorySyncAuditAppendInput, append_memory_import_audit},
    sync_encryption::plaintext_memory_import_bundle,
    sync_signature::{
        MEMORY_EXPORT_SIGNATURE_STATUS_UNSIGNED, MemoryExportSignatureView,
        verify_memory_export_signature,
    },
};

const MEMORY_IMPORT_STATUS_VERIFIED: &str = "memory_import_verified";
const MEMORY_IMPORT_STATUS_APPLIED: &str = "memory_import_applied";
const MEMORY_IMPORT_ACTION_ACCEPT_IMPORTED: &str = "accept_imported";
const MEMORY_IMPORT_ACTION_KEEP_LOCAL: &str = "keep_local";
const MEMORY_IMPORT_REASON_NO_LOCAL_RECORD: &str = "no_local_record";
const MEMORY_IMPORT_REASON_IDENTICAL_RECORD: &str = "identical_record";
const MEMORY_IMPORT_REASON_LOCAL_CONFLICT: &str = "local_conflict";

/// `axon memory import` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryImportCommand {
    /// 待读取的 Memory ref-only 导出包。
    #[arg(long)]
    pub input: PathBuf,
    /// 显式本机应用无冲突导入计划。
    #[arg(long)]
    pub yes: bool,
    /// 可信 Ed25519 公钥十六进制；导入签名包时必须显式提供。
    #[arg(long)]
    pub trusted_public_key_hex: Option<String>,
    /// XChaCha20-Poly1305 32 字节解密 key 十六进制；仅用于本次导入，不会写入本机 store。
    #[arg(long)]
    pub decryption_key_hex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryImportReport {
    status: &'static str,
    schema_version: u32,
    record_count: usize,
    payload_digest_algorithm: String,
    payload_digest: String,
    signature_status: String,
    dry_run: bool,
    records_imported: usize,
    conflict_count: usize,
    conflicts: Vec<MemoryImportConflictView>,
    plan_items: Vec<MemoryImportPlanItemView>,
    ref_only: bool,
    remote_write_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryImportConflictView {
    memory_id: u64,
    imported_state: Option<String>,
    local_state: String,
    imported_content_ref: Option<String>,
    local_content_ref: Option<String>,
    imported_evidence_ref: Option<String>,
    local_evidence_ref: Option<String>,
    recommended_action: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryImportPlanItemView {
    memory_id: u64,
    action: &'static str,
    reason: &'static str,
    requires_review: bool,
    imported_state: Option<String>,
    local_state: Option<String>,
    imported_content_ref: Option<String>,
    local_content_ref: Option<String>,
    imported_evidence_ref: Option<String>,
    local_evidence_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryImportBundle {
    schema_version: u32,
    record_count: usize,
    payload_digest_algorithm: String,
    payload_digest: String,
    signature_status: String,
    signature: Option<MemoryExportSignatureView>,
    records: Vec<serde_json::Value>,
    memory_summary_exported: bool,
    prompt_text_exported: bool,
    provider_raw_payload_exported: bool,
    remote_write_allowed: bool,
}

/// 执行 Memory ref-only 导入预检或应用。
#[must_use]
pub fn run_memory_import(
    format: MemoryOpsOutputFormat,
    context: &MemoryOpsContext,
    command: &MemoryImportCommand,
) -> MemoryOpsResult {
    let Ok(text) = fs::read_to_string(&command.input) else {
        return cli_error("memory import 输入文件读取失败");
    };
    let text = match plaintext_memory_import_bundle(&text, command.decryption_key_hex.as_deref()) {
        Ok(text) => text,
        Err(error) => return cli_error(error),
    };
    let Ok(bundle) = serde_json::from_str::<MemoryImportBundle>(&text) else {
        return cli_error("memory import 输入 JSON 无效");
    };
    let report = match verify_bundle(
        context,
        bundle,
        command.yes,
        command.trusted_public_key_hex.as_deref(),
    ) {
        Ok(report) => report,
        Err(error) => return cli_error(error),
    };
    if let Err(error) = append_memory_import_audit(
        context,
        MemorySyncAuditAppendInput {
            status: report.status,
            record_count: report.record_count,
            records_imported: report.records_imported,
            conflict_count: report.conflict_count,
            payload_digest_algorithm: report.payload_digest_algorithm.clone(),
            payload_digest: report.payload_digest.clone(),
            signature_status: report.signature_status.clone(),
            dry_run: report.dry_run,
            input_path: Some(command.input.clone()),
            output_path: None,
        },
    ) {
        return cli_error(error);
    }
    render_import(format, &report)
}

fn verify_bundle(
    context: &MemoryOpsContext,
    bundle: MemoryImportBundle,
    apply: bool,
    trusted_public_key_hex: Option<&str>,
) -> Result<MemoryImportReport, &'static str> {
    if bundle.schema_version != MEMORY_EXPORT_SCHEMA_VERSION
        || bundle.payload_digest_algorithm != MEMORY_EXPORT_DIGEST_ALGORITHM
    {
        return Err("memory import 导出包治理元数据不受支持");
    }
    if bundle.record_count != bundle.records.len() {
        return Err("memory import 记录数量与导出包不一致");
    }
    if !bundle.memory_summary_exported
        || bundle.prompt_text_exported
        || bundle.provider_raw_payload_exported
        || bundle.remote_write_allowed
    {
        return Err("memory import 只接受包含脱敏摘要、ref-only 且远控关闭的导出包");
    }
    let digest = records_digest(&bundle.records).map_err(|_| "memory import digest 计算失败")?;
    if digest != bundle.payload_digest {
        return Err("memory import payloadDigest 校验失败");
    }
    let signature_status = verify_memory_export_signature(
        &bundle.payload_digest,
        bundle.signature.as_ref(),
        trusted_public_key_hex,
    )?;
    if signature_status != bundle.signature_status
        || (bundle.signature.is_none()
            && bundle.signature_status != MEMORY_EXPORT_SIGNATURE_STATUS_UNSIGNED)
    {
        return Err("memory import 签名状态与导出包不一致");
    }
    let import_records = import_record_refs(&bundle.records)?;
    let plan_items = import_plan_items(context, &import_records)?;
    let conflicts = plan_conflicts(&plan_items);
    if apply && !conflicts.is_empty() {
        return Err("memory import 存在本机冲突，需先完成人工复核");
    }
    let records_imported = if apply {
        apply_import_plan(context, &import_records, &plan_items)?
    } else {
        0
    };
    Ok(MemoryImportReport {
        status: if apply {
            MEMORY_IMPORT_STATUS_APPLIED
        } else {
            MEMORY_IMPORT_STATUS_VERIFIED
        },
        schema_version: bundle.schema_version,
        record_count: bundle.records.len(),
        payload_digest_algorithm: bundle.payload_digest_algorithm,
        payload_digest: bundle.payload_digest,
        signature_status: signature_status.to_owned(),
        dry_run: !apply,
        records_imported,
        conflict_count: conflicts.len(),
        conflicts,
        plan_items,
        ref_only: true,
        remote_write_allowed: false,
    })
}

fn import_plan_items(
    context: &MemoryOpsContext,
    records: &[ImportRecordRef],
) -> Result<Vec<MemoryImportPlanItemView>, &'static str> {
    let store = context
        .orchestrator
        .sqlite_store()
        .map_err(|_| "memory import 本机 SQLite 读取失败")?;
    records
        .iter()
        .map(|record| {
            let local = store
                .get_record(axon_domain::MemoryId(record.memory_id))
                .map_err(|_| "memory import 本机记录读取失败")?;
            Ok(import_plan_item(record, local.as_ref()))
        })
        .collect()
}

fn import_plan_item(
    record: &ImportRecordRef,
    local: Option<&axon_agent_runtime::MemorySqliteRecord>,
) -> MemoryImportPlanItemView {
    let local_content_ref = local.and_then(|local| local.record.content_ref.clone());
    let local_evidence_ref = local.and_then(|local| {
        local
            .record
            .source_trace
            .as_ref()
            .and_then(|trace| trace.evidence_ref.clone())
    });
    let local_state = local.map(|local| memory_state_code(local.record.state).to_owned());
    let differs = local_state.as_ref().is_some_and(|state| {
        Some(state.as_str()) != record.state.as_deref()
            || local_content_ref != record.content_ref
            || local_evidence_ref != record.evidence_ref
    });
    let (action, reason, requires_review) = if differs {
        (
            MEMORY_IMPORT_ACTION_KEEP_LOCAL,
            MEMORY_IMPORT_REASON_LOCAL_CONFLICT,
            true,
        )
    } else if local_state.is_some() {
        (
            MEMORY_IMPORT_ACTION_ACCEPT_IMPORTED,
            MEMORY_IMPORT_REASON_IDENTICAL_RECORD,
            false,
        )
    } else {
        (
            MEMORY_IMPORT_ACTION_ACCEPT_IMPORTED,
            MEMORY_IMPORT_REASON_NO_LOCAL_RECORD,
            false,
        )
    };

    MemoryImportPlanItemView {
        memory_id: record.memory_id,
        action,
        reason,
        requires_review,
        imported_state: record.state.clone(),
        local_state,
        imported_content_ref: record.content_ref.clone(),
        local_content_ref,
        imported_evidence_ref: record.evidence_ref.clone(),
        local_evidence_ref,
    }
}

fn plan_conflicts(plan_items: &[MemoryImportPlanItemView]) -> Vec<MemoryImportConflictView> {
    plan_items
        .iter()
        .filter(|item| item.requires_review)
        .map(|item| MemoryImportConflictView {
            memory_id: item.memory_id,
            imported_state: item.imported_state.clone(),
            local_state: item.local_state.clone().unwrap_or_default(),
            imported_content_ref: item.imported_content_ref.clone(),
            local_content_ref: item.local_content_ref.clone(),
            imported_evidence_ref: item.imported_evidence_ref.clone(),
            local_evidence_ref: item.local_evidence_ref.clone(),
            recommended_action: item.action,
        })
        .collect()
}

fn apply_import_plan(
    context: &MemoryOpsContext,
    records: &[ImportRecordRef],
    plan_items: &[MemoryImportPlanItemView],
) -> Result<usize, &'static str> {
    let store = context
        .orchestrator
        .sqlite_store()
        .map_err(|_| "memory import 本机 SQLite 写入失败")?;
    let mut imported = 0usize;
    for item in plan_items.iter().filter(|item| {
        item.action == MEMORY_IMPORT_ACTION_ACCEPT_IMPORTED
            && item.reason == MEMORY_IMPORT_REASON_NO_LOCAL_RECORD
    }) {
        let record = records
            .iter()
            .find(|record| record.memory_id == item.memory_id)
            .ok_or("memory import 应用计划缺少记录")?;
        let sqlite_record = sqlite_record_from_import(record)?;
        store
            .upsert_record(&sqlite_record)
            .map_err(|_| "memory import 本机 SQLite 写入失败")?;
        upsert_text_projection(context, &sqlite_record)?;
        imported = imported.saturating_add(1);
    }
    Ok(imported)
}

fn upsert_text_projection(
    context: &MemoryOpsContext,
    record: &MemorySqliteRecord,
) -> Result<(), &'static str> {
    let source = record.record.source_trace.as_ref();
    context
        .orchestrator
        .text_index()
        .and_then(|index| {
            index.upsert_document(&MemoryTextDocument {
                memory_id: record.record.id,
                content_ref: record.record.content_ref.clone(),
                evidence_ref: source.and_then(|trace| trace.evidence_ref.clone()),
                tombstone_ref: record
                    .record
                    .tombstone
                    .as_ref()
                    .map(|tombstone| tombstone.blocked_ref.clone()),
                memory_summary: source.map(|trace| trace.summary.clone()),
            })
        })
        .map_err(|_| "memory import Tantivy 投影写入失败")
}

fn records_digest(records: &[serde_json::Value]) -> Result<String, serde_json::Error> {
    let value = serde_json::to_value(records)?;
    let payload = serde_json::to_vec(&value)?;
    Ok(sha256_tagged_hex(&payload))
}

fn render_import(format: MemoryOpsOutputFormat, report: &MemoryImportReport) -> MemoryOpsResult {
    match format {
        MemoryOpsOutputFormat::Json => json_result(report),
        MemoryOpsOutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "Memory import: status={} records={} imported={} conflicts={} dry_run={} digest={}",
                report.status,
                report.record_count,
                report.records_imported,
                report.conflict_count,
                report.dry_run,
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
