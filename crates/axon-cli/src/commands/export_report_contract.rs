use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportReport {
    pub(super) status: &'static str,
    pub(super) schema_version: u32,
    pub(super) generated_at_ms: u64,
    pub(super) config_path: String,
    pub(super) axon_root: String,
    pub(super) limit: usize,
    pub(super) ref_only: bool,
    pub(super) raw_prompt_exported: bool,
    pub(super) raw_response_exported: bool,
    pub(super) provider_raw_payload_exported: bool,
    pub(super) request_body_exported: bool,
    pub(super) remote_trigger_allowed: bool,
    pub(super) sources: Vec<ExportSourceView>,
    pub(super) sessions: Vec<ExportSessionView>,
    pub(super) memory: ExportMemoryView,
    pub(super) mcp: ExportMcpView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportSourceView {
    pub(super) name: &'static str,
    pub(super) path: String,
    pub(super) exists: bool,
    pub(super) record_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportSessionView {
    pub(super) session_id: String,
    pub(super) parent_session_id: Option<String>,
    pub(super) created_at_ms: u64,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) prompt_sha256: String,
    pub(super) response_sha256: String,
    pub(super) prompt_chars: usize,
    pub(super) response_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportMemoryView {
    pub(super) status: &'static str,
    pub(super) store_path: String,
    pub(super) total_records: usize,
    pub(super) active_records: usize,
    pub(super) archived_records: usize,
    pub(super) vector_records: usize,
    pub(super) prompt_text_exported: bool,
    pub(super) raw_payload_exported: bool,
    pub(super) remote_write_allowed: bool,
    pub(super) records_truncated: bool,
    pub(super) records: Vec<ExportMemoryRecordView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportMemoryRecordView {
    pub(super) memory_id: u64,
    pub(super) candidate_id: u64,
    pub(super) memory_kind: &'static str,
    pub(super) state: &'static str,
    pub(super) content_ref: Option<String>,
    pub(super) evidence_ref: Option<String>,
    pub(super) has_source_summary_ref: bool,
    pub(super) created_at_ms: u64,
    pub(super) updated_at_ms: u64,
    pub(super) confidence_basis_points: u16,
    pub(super) importance_basis_points: u16,
    pub(super) vector_indexed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportMcpView {
    pub(super) status: &'static str,
    pub(super) pool_store_path: String,
    pub(super) total_records: usize,
    pub(super) servers: Vec<ExportMcpServerView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportMcpServerView {
    pub(super) server_id: String,
    pub(super) recorded_at_ms: u64,
    pub(super) active_session_count: usize,
    pub(super) busy_session_count: usize,
    pub(super) failure_count: u32,
    pub(super) doctor_status: String,
    pub(super) terminal_main_loop_continues: bool,
    pub(super) remote_trigger_allowed: bool,
}
