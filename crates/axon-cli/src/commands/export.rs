use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use axon_agent_runtime::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, McpPoolStoreRecord, McpPoolStoreRuntime,
    MemoryConfig as AxonMemoryConfig, MemoryOrchestrator, MemorySqliteRecord,
    MemorySqliteRecordFilter,
};
use axon_config::{AxonConfig, ConfigLoader};
use axon_crypto::sha256_hex;
use axon_display::display_path;
use axon_domain::MemoryState;
use axon_session::{JsonlThreadEventStore, ThreadDetail, ThreadHistoryItem, ThreadService};
use axon_time::unix_now_ms;
use clap::Args;

use crate::{
    CliRunResult, OutputFormat,
    export_report_contract::{
        ExportMcpServerView, ExportMcpView, ExportMemoryRecordView, ExportMemoryView, ExportReport,
        ExportSessionView, ExportSourceView,
    },
    management_commands::{cli_config_error, cli_error, json_result, resolved_config_path},
    runtime_paths::runtime_axon_root_path,
};

const EXPORT_SCHEMA_VERSION: u32 = 1;
const EXPORT_DEFAULT_LIMIT: usize = 20;
const EXPORT_MAX_LIMIT: usize = 100;

/// `axon export` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ExportCommand {
    /// 写入 JSON 文件；省略时输出到 stdout。
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// 每类明细最多导出条数。
    #[arg(long)]
    pub limit: Option<usize>,
}

pub(crate) fn run_export(
    command: &ExportCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let loaded = match ConfigLoader::load_from_path(&path) {
        Ok(loaded) => loaded,
        Err(error) => return cli_config_error(&error),
    };
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let report = match ExportReport::collect(&path, &loaded.config, limit) {
        Ok(report) => report,
        Err(error) => return cli_error(error),
    };
    if let Some(output) = command.output.as_ref() {
        return write_report(format, output, &report);
    }
    render_report(format, &report)
}

impl ExportReport {
    fn collect(config_path: &Path, config: &AxonConfig, limit: usize) -> Result<Self, String> {
        let axon_root = runtime_axon_root_path(config_path)?;
        let session_path = JsonlThreadEventStore::path(&axon_root);
        let mcp_pool_path = McpPoolStoreRuntime::resolve_store_path(&axon_root);
        let memory_store_path = memory_store_path(&axon_root, config)?;
        let sessions = read_session_views(&axon_root, limit)?;
        let memory = export_memory(&axon_root, config, limit)?;
        let mcp = export_mcp(&mcp_pool_path, limit)?;
        let sources = vec![
            source_view("sessions", &session_path)?,
            source_view_with_count("memory", &memory_store_path, memory.total_records),
            source_view_with_count("mcp", &mcp_pool_path, mcp.total_records),
        ];
        Ok(Self {
            status: "export_ready",
            schema_version: EXPORT_SCHEMA_VERSION,
            generated_at_ms: now_ms(),
            config_path: display_path(config_path),
            axon_root: display_path(&axon_root),
            limit,
            ref_only: true,
            raw_prompt_exported: false,
            raw_response_exported: false,
            provider_raw_payload_exported: false,
            request_body_exported: false,
            remote_trigger_allowed: false,
            sources,
            sessions,
            memory,
            mcp,
        })
    }
}

fn read_session_views(root: &Path, limit: usize) -> Result<Vec<ExportSessionView>, String> {
    let service = ThreadService::new(JsonlThreadEventStore);
    let sessions = service.list_threads(root, limit)?;
    Ok(sessions
        .into_iter()
        .map(|summary| service.read_thread(root, &summary.thread_id))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .take(limit)
        .map(export_session_view)
        .collect())
}

fn export_session_view(detail: ThreadDetail) -> ExportSessionView {
    let last_turn = detail.turns.last();
    let prompt = last_turn.and_then(turn_user_content).unwrap_or_default();
    let response = last_turn
        .and_then(turn_assistant_content)
        .unwrap_or_default();
    ExportSessionView {
        session_id: detail.thread.thread_id,
        parent_session_id: detail.thread.parent_thread_id,
        created_at_ms: detail.thread.created_at_ms,
        provider: detail.thread.settings.provider,
        model: detail.thread.settings.model,
        prompt_sha256: sha256_hex(prompt),
        response_sha256: sha256_hex(response),
        prompt_chars: prompt.chars().count(),
        response_chars: response.chars().count(),
    }
}

fn turn_user_content(turn: &axon_session::Turn) -> Option<&str> {
    turn.history_items.iter().find_map(|item| match item {
        ThreadHistoryItem::UserMessage { content, .. } => Some(content.as_str()),
        _ => None,
    })
}

fn turn_assistant_content(turn: &axon_session::Turn) -> Option<&str> {
    turn.history_items.iter().rev().find_map(|item| match item {
        ThreadHistoryItem::AssistantMessage { content, .. } => Some(content.as_str()),
        _ => None,
    })
}

fn export_memory(
    root: &Path,
    config: &AxonConfig,
    limit: usize,
) -> Result<ExportMemoryView, String> {
    let memory_config = AxonMemoryConfig::from_config_dir(
        root,
        config.memory.embedding.enabled,
        memory_embedding_dimension(config),
    )
    .map_err(|error| format!("{error:?}"))?;
    let store_path = memory_config.db_path.clone();
    let orchestrator =
        MemoryOrchestrator::init(memory_config).map_err(|error| format!("{error:?}"))?;
    let store = orchestrator
        .sqlite_store()
        .map_err(|error| format!("{error:?}"))?;
    let summary = store.summary().map_err(|error| format!("{error:?}"))?;
    let records = store
        .records_limited(MemorySqliteRecordFilter::All, limit)
        .map_err(|error| format!("{error:?}"))?;
    Ok(ExportMemoryView {
        status: "memory_ready",
        store_path: display_path(&store_path),
        total_records: summary.total_records,
        active_records: summary.active_records,
        archived_records: summary.archived_records,
        vector_records: summary.vector_records,
        prompt_text_exported: false,
        raw_payload_exported: false,
        remote_write_allowed: summary.remote_write_allowed,
        records_truncated: summary.total_records > records.len(),
        records: records.iter().map(memory_record_view).collect(),
    })
}

fn memory_store_path(root: &Path, config: &AxonConfig) -> Result<PathBuf, String> {
    AxonMemoryConfig::from_config_dir(
        root,
        config.memory.embedding.enabled,
        memory_embedding_dimension(config),
    )
    .map(|config| config.db_path)
    .map_err(|error| format!("{error:?}"))
}

fn export_mcp(path: &Path, limit: usize) -> Result<ExportMcpView, String> {
    let mut records =
        McpPoolStoreRuntime::read_records(path).map_err(|error| format!("{error:?}"))?;
    records.sort_by(|left, right| {
        right
            .recorded_at_ms
            .cmp(&left.recorded_at_ms)
            .then_with(|| left.server_id.cmp(&right.server_id))
    });
    let total_records = records.len();
    Ok(ExportMcpView {
        status: "mcp_ready",
        pool_store_path: display_path(path),
        total_records,
        servers: records.iter().take(limit).map(mcp_server_view).collect(),
    })
}

fn render_report(format: OutputFormat, report: &ExportReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = format!(
                "Axon export: {}\n- root: {}\n- ref_only: {}\n",
                report.status, report.axon_root, report.ref_only
            );
            for source in &report.sources {
                let _ = writeln!(
                    stdout,
                    "- {}: records={} exists={} path={}",
                    source.name, source.record_count, source.exists, source.path
                );
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

fn write_report(format: OutputFormat, path: &Path, report: &ExportReport) -> CliRunResult {
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return cli_error(format!("无法创建导出目录：{error}"));
    }
    let Ok(content) = serde_json::to_string_pretty(report) else {
        return cli_error("导出报告序列化失败");
    };
    if let Err(error) = fs::write(path, format!("{content}\n")) {
        return cli_error(format!("无法写入导出文件：{error}"));
    }
    match format {
        OutputFormat::Json => json_result(&serde_json::json!({
            "status": "export_written",
            "outputPath": display_path(path),
            "refOnly": true,
            "schemaVersion": EXPORT_SCHEMA_VERSION,
        })),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!("已写入 ref-only 导出文件: {}\n", path.display()),
            stderr: String::new(),
        },
    }
}

fn source_view(name: &'static str, path: &Path) -> Result<ExportSourceView, String> {
    Ok(ExportSourceView {
        name,
        path: display_path(path),
        exists: path.exists(),
        record_count: count_jsonl_records(path)?,
    })
}

fn source_view_with_count(
    name: &'static str,
    path: &Path,
    record_count: usize,
) -> ExportSourceView {
    ExportSourceView {
        name,
        path: display_path(path),
        exists: path.exists(),
        record_count,
    }
}

fn count_jsonl_records(path: &Path) -> Result<usize, String> {
    if !path.exists() {
        return Ok(0);
    }
    let text = fs::read_to_string(path).map_err(|_| "无法读取导出源".to_owned())?;
    Ok(text.lines().filter(|line| !line.trim().is_empty()).count())
}

fn memory_record_view(record: &MemorySqliteRecord) -> ExportMemoryRecordView {
    let source_trace = record.record.source_trace.as_ref();
    ExportMemoryRecordView {
        memory_id: record.record.id.0,
        candidate_id: record.record.candidate_id.0,
        memory_kind: record.memory_kind.as_str(),
        state: memory_state_code(record.record.state),
        content_ref: record.record.content_ref.clone(),
        evidence_ref: source_trace.and_then(|trace| trace.evidence_ref.clone()),
        has_source_summary_ref: source_trace.is_some_and(|trace| !trace.summary.trim().is_empty()),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
        confidence_basis_points: record.confidence_basis_points,
        importance_basis_points: record.importance_basis_points,
        vector_indexed: record.vector_id.is_some(),
    }
}

fn mcp_server_view(record: &McpPoolStoreRecord) -> ExportMcpServerView {
    ExportMcpServerView {
        server_id: record.server_id.clone(),
        recorded_at_ms: record.recorded_at_ms,
        active_session_count: record.active_session_count,
        busy_session_count: record.busy_session_count,
        failure_count: record.failure_count,
        doctor_status: record.doctor_status.clone(),
        terminal_main_loop_continues: record.terminal_main_loop_continues,
        remote_trigger_allowed: false,
    }
}

fn bounded_limit(limit: Option<usize>) -> Result<usize, &'static str> {
    let limit = limit.unwrap_or(EXPORT_DEFAULT_LIMIT);
    if limit == 0 || limit > EXPORT_MAX_LIMIT {
        return Err("export limit 必须在 1..=100 范围内");
    }
    Ok(limit)
}

fn memory_embedding_dimension(config: &AxonConfig) -> usize {
    if config.memory.embedding.enabled {
        config.memory.embedding.dimension
    } else {
        MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION
    }
}

fn now_ms() -> u64 {
    unix_now_ms()
}

const fn memory_state_code(state: MemoryState) -> &'static str {
    match state {
        MemoryState::Weak => "weak",
        MemoryState::Strong => "strong",
        MemoryState::Quarantined => "quarantined",
        MemoryState::Tombstone => "tombstone",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_session::{
        Thread, ThreadHistoryItem, ThreadSettings, ThreadSource, ThreadStatus, Turn, TurnStatus,
    };

    #[test]
    fn export_session_view_is_ref_only_for_structured_tool_history() {
        let detail = ThreadDetail {
            thread: Thread {
                thread_id: "session-export".to_owned(),
                session_root_id: "session-export".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                name: None,
                status: ThreadStatus::Idle,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: "E:/fixture".to_owned(),
                    provider: "fixture".to_owned(),
                    model: "fixture".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: 1,
                updated_at_ms: 1,
                archived_at_ms: None,
            },
            turns: vec![Turn {
                turn_id: "turn-1".to_owned(),
                sequence: 0,
                status: TurnStatus::Completed,
                history_items: vec![
                    ThreadHistoryItem::UserMessage {
                        sequence: 0,
                        part_id: "user-1".to_owned(),
                        content: "原始 prompt 不应导出".to_owned(),
                        local_images: Vec::new(),
                        local_files: Vec::new(),
                        provider_metadata: None,
                    },
                    ThreadHistoryItem::ToolCall {
                        sequence: 1,
                        provider_turn_id: 1,
                        part_id: "tool-1".to_owned(),
                        tool_call_id: "1".to_owned(),
                        operation_id: "op-1".to_owned(),
                        provider_call_id: "provider-call-1".to_owned(),
                        tool_name: "mcp.search".to_owned(),
                        input: r#"{"secret":"原始参数"}"#.into(),
                        status: axon_session::ThreadHistoryStatus::Completed,
                        provider_metadata: Some(serde_json::json!({"raw":"provider payload"})),
                    },
                    ThreadHistoryItem::AssistantMessage {
                        sequence: 2,
                        part_id: "assistant-1".to_owned(),
                        content: "原始 response 不应导出".to_owned(),
                        provider_metadata: None,
                    },
                ],
                token_usage_json: None,
                started_at_ms: 1,
                completed_at_ms: Some(1),
                interrupted_at_ms: None,
                failure: None,
            }],
            compacted_history: None,
        };

        let view = export_session_view(detail);
        let serialized = serde_json::to_string(&view).expect("导出视图必须可序列化");
        assert!(!serialized.contains("原始 prompt"));
        assert!(!serialized.contains("原始参数"));
        assert!(!serialized.contains("原始 response"));
        assert!(!serialized.contains("provider payload"));
        assert_eq!(view.prompt_sha256, sha256_hex("原始 prompt 不应导出"));
        assert_eq!(view.response_sha256, sha256_hex("原始 response 不应导出"));
        assert_eq!(view.prompt_chars, "原始 prompt 不应导出".chars().count());
        assert_eq!(
            view.response_chars,
            "原始 response 不应导出".chars().count()
        );
    }
}
