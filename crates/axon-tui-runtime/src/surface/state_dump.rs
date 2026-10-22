//! TUI debug state dump schema。
//!
//! 该格式只服务调试与测试回放，不作为生产会话恢复兼容格式。

use std::{
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{
    cell_payload::{CellPayloadEnvelope, CellPayloadSchemaError},
    telemetry::TuiTelemetrySnapshot,
};

const STATE_DUMP_SCHEMA_VERSION: u16 = 1;
const REDACTED_TEXT: &str = "[REDACTED]";
const REDACTED_PATH_SEGMENT: &str = "[REDACTED_SEGMENT]";
const STATE_DUMP_DIR: &str = "axon-tui-state-dumps";
const STATE_DUMP_FILE_PREFIX: &str = "tui-state";
const SENSITIVE_PATH_MARKERS: &[&str] = &[
    ".ssh",
    ".aws",
    ".azure",
    ".config",
    ".codex",
    ".claude",
    ".agents",
    "secret",
    "secrets",
    "token",
    "tokens",
    "password",
    "private",
    "credential",
    "credentials",
];

/// TUI debug dump 根对象。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDump {
    pub(crate) schema_version: u16,
    pub(crate) redaction: TuiStateDumpRedaction,
    pub(crate) header: TuiStateDumpHeader,
    pub(crate) viewport: TuiStateDumpViewport,
    pub(crate) composer: TuiStateDumpComposer,
    pub(crate) cells: TuiStateDumpCells,
    pub(crate) bridge: TuiStateDumpBridge,
    pub(crate) telemetry: TuiTelemetrySnapshot,
}

/// dump 的脱敏策略摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpRedaction {
    pub(crate) enabled: bool,
    pub(crate) prompt_redacted: bool,
    pub(crate) path_segments_redacted: bool,
}

/// Header debug dump。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpHeader {
    pub(crate) cwd: String,
    pub(crate) workspace_instruction_paths: Vec<String>,
    pub(crate) config_provider: String,
    pub(crate) config_model: String,
    pub(crate) config_auth_status: String,
    pub(crate) hint: String,
    pub(crate) plan_progress: Option<TuiStateDumpPlanProgress>,
    pub(crate) session_usage_total: u64,
    pub(crate) last_turn_usage_total: u64,
    pub(crate) live_turn_usage_total: u64,
    pub(crate) lsp_active_count: usize,
    pub(crate) gateway_active_turn_count: usize,
}

/// Plan 进度 debug dump。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpPlanProgress {
    pub(crate) completed: usize,
    pub(crate) total: usize,
}

/// Viewport debug dump。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpViewport {
    pub(crate) active_dock_kind: Option<String>,
    pub(crate) transcript_view_offset_rows: usize,
    pub(crate) max_transcript_view_offset_rows: usize,
    pub(crate) transcript_view_page_rows: usize,
    pub(crate) transcript_view_scroll_state: String,
    pub(crate) transcript_view_jump_available: bool,
    pub(crate) transcript_view_jump_distance_rows: usize,
    pub(crate) transcript_view_jump_threshold_rows: usize,
    pub(crate) transcript_view_top_anchor_cell_id: Option<u64>,
    pub(crate) transcript_view_top_anchor_row_offset: Option<usize>,
    pub(crate) selected_runtime_render_index: Option<usize>,
    pub(crate) selected_runtime_part_id: Option<String>,
    pub(crate) tool_output_dock_open: bool,
}

/// Composer debug dump。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpComposer {
    pub(crate) draft: String,
    pub(crate) draft_chars: usize,
    pub(crate) submitted_prompt: Option<String>,
    pub(crate) command_suggestions_count: usize,
    pub(crate) selected_command_suggestion: usize,
    pub(crate) composer_popup_open: bool,
    pub(crate) history_count: usize,
    pub(crate) history_path: String,
    pub(crate) history_index: Option<usize>,
    pub(crate) queued_user_messages: usize,
    pub(crate) pending_steers: usize,
    pub(crate) user_turn_pending_start: bool,
}

/// Cell store debug dump。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpCells {
    pub(crate) transcript: Vec<TuiStateDumpCell>,
    pub(crate) active_runtime_parts: Vec<TuiStateDumpRuntimePartAnchor>,
    pub(crate) tool_output_count: usize,
}

/// 单个 transcript cell debug dump。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpCell {
    pub(crate) render_id: u64,
    pub(crate) part_id: String,
    pub(crate) semantic_part_id: Option<String>,
    pub(crate) revision: u64,
    pub(crate) role: String,
    pub(crate) mode: String,
    pub(crate) action: String,
    pub(crate) text: String,
    pub(crate) payload: Option<CellPayloadEnvelope>,
}

/// Runtime bridge debug dump。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpBridge {
    pub(crate) runtime_status: String,
    pub(crate) runtime_tmp_dir: String,
    pub(crate) assistant_stream_count: usize,
    pub(crate) active_assistant_anchors: Vec<TuiStateDumpRuntimePartAnchor>,
    pub(crate) active_reasoning_parts: Vec<TuiStateDumpRuntimePartState>,
    pub(crate) has_live_plan: bool,
    pub(crate) has_live_review: bool,
    pub(crate) has_live_lifecycle: bool,
    pub(crate) has_live_hook: bool,
    pub(crate) interrupt_requested: bool,
    pub(crate) should_exit: bool,
    pub(crate) runtime_submission_enabled: bool,
}

/// Runtime part anchor debug ref。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpRuntimePartAnchor {
    pub(crate) part_id: String,
    pub(crate) cell_index: usize,
}

/// Runtime part active-state debug ref。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TuiStateDumpRuntimePartState {
    pub(crate) part_id: String,
    pub(crate) anchor: Option<TuiStateDumpRuntimePartAnchor>,
}

/// state dump load 错误。
#[derive(Debug)]
pub(crate) enum TuiStateDumpLoadError {
    Json(serde_json::Error),
    SchemaVersion { expected: u16, actual: u16 },
    CellPayload(CellPayloadSchemaError),
    CellPartIdMismatch { part_id: String, payload_id: String },
}

impl TuiStateDump {
    pub(crate) fn new(
        header: TuiStateDumpHeader,
        viewport: TuiStateDumpViewport,
        composer: TuiStateDumpComposer,
        cells: TuiStateDumpCells,
        bridge: TuiStateDumpBridge,
        telemetry: TuiTelemetrySnapshot,
    ) -> Self {
        Self {
            schema_version: STATE_DUMP_SCHEMA_VERSION,
            redaction: TuiStateDumpRedaction {
                enabled: true,
                prompt_redacted: true,
                path_segments_redacted: true,
            },
            header,
            viewport,
            composer,
            cells,
            bridge,
            telemetry,
        }
    }

    pub(crate) fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub(crate) fn load_json(json: &str) -> Result<Self, TuiStateDumpLoadError> {
        let dump: Self = serde_json::from_str(json).map_err(TuiStateDumpLoadError::Json)?;
        if dump.schema_version != STATE_DUMP_SCHEMA_VERSION {
            return Err(TuiStateDumpLoadError::SchemaVersion {
                expected: STATE_DUMP_SCHEMA_VERSION,
                actual: dump.schema_version,
            });
        }
        for cell in &dump.cells.transcript {
            if let Some(payload) = &cell.payload {
                let validated = payload
                    .clone()
                    .validate()
                    .map_err(TuiStateDumpLoadError::CellPayload)?;
                validate_cell_part_id(cell, &validated)?;
            }
        }
        Ok(dump)
    }
}

impl fmt::Display for TuiStateDumpLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "state dump JSON 无效: {error}"),
            Self::SchemaVersion { expected, actual } => {
                write!(
                    formatter,
                    "state dump schema version 不匹配: expected={expected}, actual={actual}"
                )
            }
            Self::CellPayload(error) => write!(formatter, "state dump cell payload 无效: {error}"),
            Self::CellPartIdMismatch {
                part_id,
                payload_id,
            } => write!(
                formatter,
                "state dump cell part_id 不一致: part_id={part_id}, payload_id={payload_id}"
            ),
        }
    }
}

impl std::error::Error for TuiStateDumpLoadError {}

pub(crate) fn redact_prompt(value: &str) -> String {
    if value.trim().is_empty() {
        return String::new();
    }
    REDACTED_TEXT.to_owned()
}

pub(crate) fn redact_optional_prompt(value: Option<&str>) -> Option<String> {
    value.map(redact_prompt)
}

pub(crate) fn redact_path(value: impl AsRef<Path>) -> String {
    let path = value.as_ref();
    let mut parts = Vec::new();
    for component in path.components() {
        let text = component.as_os_str().to_string_lossy();
        if is_sensitive_path_segment(&text) {
            parts.push(REDACTED_PATH_SEGMENT.to_owned());
        } else {
            parts.push(text.to_string());
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    parts.join(std::path::MAIN_SEPARATOR_STR)
}

pub(crate) fn state_dump_path(transaction: &str) -> PathBuf {
    let safe_transaction = transaction
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let filename = format!(
        "{STATE_DUMP_FILE_PREFIX}-{safe_transaction}-{}.json",
        std::process::id()
    );
    std::env::temp_dir().join(STATE_DUMP_DIR).join(filename)
}

fn is_sensitive_path_segment(segment: &str) -> bool {
    let normalized = segment.to_ascii_lowercase();
    SENSITIVE_PATH_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn validate_cell_part_id(
    cell: &TuiStateDumpCell,
    payload: &CellPayloadEnvelope,
) -> Result<(), TuiStateDumpLoadError> {
    let Some(payload_id) = payload_identity(payload) else {
        return Ok(());
    };
    let expected = cell.semantic_part_id.as_deref().unwrap_or(&cell.part_id);
    if payload_id == expected {
        return Ok(());
    }
    Err(TuiStateDumpLoadError::CellPartIdMismatch {
        part_id: expected.to_owned(),
        payload_id: payload_id.to_owned(),
    })
}

fn payload_identity(payload: &CellPayloadEnvelope) -> Option<&str> {
    payload.payload.get("id")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::cell_payload::CellPayloadEnvelope;
    use crate::surface::telemetry::TuiRenderProfilerSnapshot;
    use serde_json::json;

    #[test]
    fn redaction_removes_prompt_and_sensitive_path_segments() {
        let path = PathBuf::from(r"C:\Users\dev\.codex\secrets\project");

        assert_eq!(redact_prompt("token=abc"), REDACTED_TEXT);
        assert!(redact_path(path).contains(REDACTED_PATH_SEGMENT));
    }

    #[test]
    fn load_validates_schema_and_payloads() {
        let dump = minimal_dump();

        let json = dump.to_json().expect("dump should serialize");
        let loaded = TuiStateDump::load_json(&json).expect("dump should load");

        assert_eq!(loaded, dump);
    }

    #[test]
    fn load_rejects_cell_payload_identity_mismatch() {
        let mut dump = minimal_dump();
        dump.cells.transcript.push(TuiStateDumpCell {
            render_id: 1,
            part_id: "reasoning-part:dump".to_owned(),
            semantic_part_id: Some("reasoning-part:dump".to_owned()),
            revision: 0,
            role: "agent".to_owned(),
            mode: "reasoning".to_owned(),
            action: "Reasoning".to_owned(),
            text: "状态转储 reasoning".to_owned(),
            payload: Some(CellPayloadEnvelope {
                kind: "reasoning".to_owned(),
                payload: json!({
                    "id": "reasoning-part:other",
                    "text": "状态转储 reasoning",
                    "done": false
                }),
            }),
        });

        let json = dump.to_json().expect("dump should serialize");
        let error = TuiStateDump::load_json(&json).expect_err("mismatch must fail");

        assert!(matches!(
            error,
            TuiStateDumpLoadError::CellPartIdMismatch { part_id, payload_id }
                if part_id == "reasoning-part:dump" && payload_id == "reasoning-part:other"
        ));
    }

    fn minimal_dump() -> TuiStateDump {
        TuiStateDump::new(
            TuiStateDumpHeader {
                cwd: "E:\\zbg\\axon-code".to_owned(),
                workspace_instruction_paths: Vec::new(),
                config_provider: "fixture".to_owned(),
                config_model: "model".to_owned(),
                config_auth_status: "ok".to_owned(),
                hint: "idle".to_owned(),
                plan_progress: None,
                session_usage_total: 0,
                last_turn_usage_total: 0,
                live_turn_usage_total: 0,
                lsp_active_count: 0,
                gateway_active_turn_count: 0,
            },
            TuiStateDumpViewport {
                active_dock_kind: None,
                transcript_view_offset_rows: 0,
                max_transcript_view_offset_rows: 0,
                transcript_view_page_rows: 1,
                transcript_view_scroll_state: "bottomAttached".to_owned(),
                transcript_view_jump_available: false,
                transcript_view_jump_distance_rows: 0,
                transcript_view_jump_threshold_rows: 40,
                transcript_view_top_anchor_cell_id: None,
                transcript_view_top_anchor_row_offset: None,
                selected_runtime_render_index: None,
                selected_runtime_part_id: None,
                tool_output_dock_open: false,
            },
            TuiStateDumpComposer {
                draft: REDACTED_TEXT.to_owned(),
                draft_chars: 12,
                submitted_prompt: Some(REDACTED_TEXT.to_owned()),
                command_suggestions_count: 0,
                selected_command_suggestion: 0,
                composer_popup_open: false,
                history_count: 0,
                history_path: String::new(),
                history_index: None,
                queued_user_messages: 0,
                pending_steers: 0,
                user_turn_pending_start: false,
            },
            TuiStateDumpCells {
                transcript: Vec::new(),
                active_runtime_parts: Vec::new(),
                tool_output_count: 0,
            },
            TuiStateDumpBridge {
                runtime_status: "ready".to_owned(),
                runtime_tmp_dir: String::new(),
                assistant_stream_count: 0,
                active_assistant_anchors: Vec::new(),
                active_reasoning_parts: Vec::new(),
                has_live_plan: false,
                has_live_review: false,
                has_live_lifecycle: false,
                has_live_hook: false,
                interrupt_requested: false,
                should_exit: false,
                runtime_submission_enabled: true,
            },
            TuiTelemetrySnapshot {
                render_profiler: TuiRenderProfilerSnapshot::default(),
                ..TuiTelemetrySnapshot::default()
            },
        )
    }
}
