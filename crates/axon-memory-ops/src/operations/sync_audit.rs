use std::path::PathBuf;

use axon_jsonl::ShardedJsonlStore;
use serde::Serialize;

use super::{MemoryOpsContext, display_path, now_ms};

const MEMORY_SYNC_AUDIT_FILE: &str = "memory_sync_audit.jsonl";
const MEMORY_SYNC_AUDIT_SCHEMA_VERSION: u32 = 1;
const MEMORY_SYNC_AUDIT_ACTION_EXPORT: &str = "export";
const MEMORY_SYNC_AUDIT_ACTION_IMPORT: &str = "import";
const MEMORY_SYNC_AUDIT_REASON_DIGEST_VERIFIED: &str = "payload_digest_verified";
const MEMORY_SYNC_AUDIT_REASON_REF_ONLY: &str = "ref_only";
const MEMORY_SYNC_AUDIT_REASON_REMOTE_WRITE_CLOSED: &str = "remote_write_closed";
const MEMORY_SYNC_AUDIT_REASON_AUDIT_PERSISTED: &str = "sync_audit_persisted";
/// Memory sync audit 使用的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

/// Memory 跨设备同步审计追加输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MemorySyncAuditAppendInput {
    pub(super) status: &'static str,
    pub(super) record_count: usize,
    pub(super) records_imported: usize,
    pub(super) conflict_count: usize,
    pub(super) payload_digest_algorithm: String,
    pub(super) payload_digest: String,
    pub(super) signature_status: String,
    pub(super) dry_run: bool,
    pub(super) input_path: Option<PathBuf>,
    pub(super) output_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemorySyncAuditRecord {
    schema_version: u32,
    recorded_at_ms: u64,
    action: String,
    status: String,
    record_count: usize,
    records_imported: usize,
    conflict_count: usize,
    payload_digest_algorithm: String,
    payload_digest: String,
    signature_status: String,
    dry_run: bool,
    input_path_ref: Option<String>,
    output_path_ref: Option<String>,
    ref_only: bool,
    remote_write_allowed: bool,
    memory_summary_exported: bool,
    prompt_text_exported: bool,
    provider_raw_payload_exported: bool,
    reasons: Vec<String>,
}

pub(super) fn append_memory_export_audit(
    context: &MemoryOpsContext,
    status: &'static str,
    record_count: usize,
    payload_digest_algorithm: impl Into<String>,
    payload_digest: impl Into<String>,
    signature_status: impl Into<String>,
    output_path: Option<PathBuf>,
) -> Result<(), String> {
    append_sync_audit(
        context,
        MEMORY_SYNC_AUDIT_ACTION_EXPORT,
        MemorySyncAuditAppendInput {
            status,
            record_count,
            records_imported: 0,
            conflict_count: 0,
            payload_digest_algorithm: payload_digest_algorithm.into(),
            payload_digest: payload_digest.into(),
            signature_status: signature_status.into(),
            dry_run: false,
            input_path: None,
            output_path,
        },
    )
}

pub(super) fn append_memory_import_audit(
    context: &MemoryOpsContext,
    input: MemorySyncAuditAppendInput,
) -> Result<(), String> {
    append_sync_audit(context, MEMORY_SYNC_AUDIT_ACTION_IMPORT, input)
}

fn append_sync_audit(
    context: &MemoryOpsContext,
    action: &'static str,
    input: MemorySyncAuditAppendInput,
) -> Result<(), String> {
    let audit_path = memory_sync_audit_path(context);
    let record = MemorySyncAuditRecord {
        schema_version: MEMORY_SYNC_AUDIT_SCHEMA_VERSION,
        recorded_at_ms: now_ms(),
        action: action.to_owned(),
        status: input.status.to_owned(),
        record_count: input.record_count,
        records_imported: input.records_imported,
        conflict_count: input.conflict_count,
        payload_digest_algorithm: input.payload_digest_algorithm,
        payload_digest: input.payload_digest,
        signature_status: input.signature_status,
        dry_run: input.dry_run,
        input_path_ref: input.input_path.as_deref().map(display_path),
        output_path_ref: input.output_path.as_deref().map(display_path),
        ref_only: true,
        remote_write_allowed: false,
        memory_summary_exported: true,
        prompt_text_exported: false,
        provider_raw_payload_exported: false,
        reasons: audit_reasons(),
    };
    let encoded = serde_json::to_string(&record)
        .map_err(|_| "memory sync audit JSON 序列化失败".to_owned())?;
    SHARD_STORE
        .append_line(audit_path, encoded.as_bytes())
        .map_err(|_| "memory sync audit 写入失败".to_owned())
}

fn memory_sync_audit_path(context: &MemoryOpsContext) -> PathBuf {
    context
        .orchestrator
        .config()
        .memory_dir
        .join(MEMORY_SYNC_AUDIT_FILE)
}

fn audit_reasons() -> Vec<String> {
    [
        MEMORY_SYNC_AUDIT_REASON_DIGEST_VERIFIED,
        MEMORY_SYNC_AUDIT_REASON_REF_ONLY,
        MEMORY_SYNC_AUDIT_REASON_REMOTE_WRITE_CLOSED,
        MEMORY_SYNC_AUDIT_REASON_AUDIT_PERSISTED,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
