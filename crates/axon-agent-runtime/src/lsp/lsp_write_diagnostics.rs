//! edit 写入后的有界 LSP diagnostics 调度。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use axon_crypto::sha256_hex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::lsp_document::path_to_file_uri;
use crate::lsp_stdio_client::{LspWorkspaceFileChange, LspWorkspaceFileChangeKind};
use crate::runtime_limits::{
    LSP_BACKGROUND_QUEUE_CAPACITY, LSP_WRITE_DIAGNOSTICS_DEFERRED_WAIT_MS,
    LSP_WRITE_DIAGNOSTICS_INLINE_WAIT_MS,
};
use crate::{LspCatalogRuntime, LspClientPoolRuntime, LspDiagnosticRequest};

const LSP_WRITE_DIAGNOSTICS_WORKER_NAME: &str = "axon-lsp-write-diagnostics";
const LSP_WRITE_DIAGNOSTICS_COLLECTOR_NAME: &str = "axon-lsp-deferred-diagnostics";
const SCHEDULE_STATUS_SCHEDULED: &str = "scheduled";
const SCHEDULE_STATUS_SKIPPED: &str = "skipped";
const SCHEDULE_STATUS_BUSY: &str = "busy";
const SCHEDULE_STATUS_UNAVAILABLE: &str = "unavailable";
const DIAGNOSTICS_STATUS_READY: &str = "ready";
const DIAGNOSTICS_STATUS_PARTIAL: &str = "partial";
const DIAGNOSTICS_STATUS_DEFERRED: &str = "deferred";
const DIAGNOSTICS_STATUS_SKIPPED: &str = "skipped";
const DIAGNOSTICS_STATUS_UNAVAILABLE: &str = "unavailable";

/// edit 写入对应的 LSP 文件变更类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspWriteFileChangeKind {
    /// 新建文件。
    Created,
    /// 修改文件。
    Changed,
    /// 删除文件。
    Deleted,
}

impl LspWriteFileChangeKind {
    const fn protocol_kind(self) -> LspWorkspaceFileChangeKind {
        match self {
            Self::Created => LspWorkspaceFileChangeKind::Created,
            Self::Changed => LspWorkspaceFileChangeKind::Changed,
            Self::Deleted => LspWorkspaceFileChangeKind::Deleted,
        }
    }
}

/// 一个已提交的 edit 文件变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspWriteFileChange {
    /// 工作区相对或绝对路径。
    pub path: PathBuf,
    /// 文件变更类型。
    pub kind: LspWriteFileChangeKind,
}

/// 单文档写后 diagnostics。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspWriteDiagnosticsDocument {
    /// 工作区相对路径。
    pub workspace_path: String,
    /// 成功响应的 LSP server。
    pub servers: Vec<String>,
    /// 原始 diagnostics 条目。
    pub diagnostics: Vec<Value>,
}

/// 延迟 diagnostics 回注报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspDeferredDiagnosticsReport {
    /// 诊断状态。
    pub status: &'static str,
    /// 已完成诊断的文档。
    pub documents: Vec<LspWriteDiagnosticsDocument>,
    /// 查询失败摘要。
    pub failures: Vec<String>,
    /// 因文件再次变化而丢弃的结果数。
    pub stale: usize,
}

impl LspDeferredDiagnosticsReport {
    /// 是否包含需要回注给模型的 diagnostics。
    #[must_use]
    pub fn has_diagnostics(&self) -> bool {
        self.documents
            .iter()
            .any(|document| !document.diagnostics.is_empty())
    }
}

/// edit 写后 diagnostics 调度结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspWriteDiagnosticsSchedule {
    /// 后台任务调度状态。
    pub status: &'static str,
    /// 已纳入任务的文件。
    pub files: Vec<String>,
    /// 与普通 LSP 后台调度回执保持一致。
    pub coalesced: usize,
    /// 与普通 LSP 后台调度回执保持一致。
    pub evicted: usize,
    /// diagnostics 当前状态。
    pub diagnostics_status: &'static str,
    /// 前台窗口内返回的文档 diagnostics。
    pub documents: Vec<LspWriteDiagnosticsDocument>,
    /// 前台窗口内可确定的失败摘要。
    pub failures: Vec<String>,
    /// 因文件再次变化而丢弃的结果数。
    pub stale: usize,
}

/// 延迟 diagnostics 回注接收器。
pub type LspDeferredDiagnosticsSink =
    Arc<dyn Fn(LspDeferredDiagnosticsReport) + Send + Sync + 'static>;

#[derive(Debug)]
struct LspWriteDiagnosticsJob {
    workspace_root: PathBuf,
    changes: Vec<NormalizedWriteChange>,
    deadline: Instant,
    completion: SyncSender<LspDeferredDiagnosticsReport>,
}

#[derive(Debug, Clone)]
struct NormalizedWriteChange {
    workspace_path: String,
    absolute_path: PathBuf,
    kind: LspWriteFileChangeKind,
    expected_sha256: Option<String>,
}

/// edit 写后 diagnostics runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct LspWriteDiagnosticsRuntime;

impl LspWriteDiagnosticsRuntime {
    /// 调度写后 LSP 同步，并仅在前台窗口内短暂等待 diagnostics。
    #[must_use]
    pub fn schedule(
        workspace_root: &Path,
        changes: impl IntoIterator<Item = LspWriteFileChange>,
        deferred_sink: Option<LspDeferredDiagnosticsSink>,
    ) -> LspWriteDiagnosticsSchedule {
        let workspace_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let changes = normalize_changes(&workspace_root, changes);
        let files = changes
            .iter()
            .map(|change| change.workspace_path.clone())
            .collect::<Vec<_>>();
        if changes.is_empty() {
            return schedule_report(SCHEDULE_STATUS_SKIPPED, files, DIAGNOSTICS_STATUS_SKIPPED);
        }
        let Some(sender) = queue() else {
            return schedule_report(
                SCHEDULE_STATUS_UNAVAILABLE,
                files,
                DIAGNOSTICS_STATUS_UNAVAILABLE,
            );
        };
        let (completion, receiver) = mpsc::sync_channel(1);
        let deadline =
            Instant::now() + Duration::from_millis(LSP_WRITE_DIAGNOSTICS_DEFERRED_WAIT_MS);
        let job = LspWriteDiagnosticsJob {
            workspace_root,
            changes,
            deadline,
            completion,
        };
        match sender.try_send(job) {
            Ok(()) => wait_inline(receiver, files, deferred_sink),
            Err(TrySendError::Full(_)) => {
                schedule_report(SCHEDULE_STATUS_BUSY, files, DIAGNOSTICS_STATUS_UNAVAILABLE)
            }
            Err(TrySendError::Disconnected(_)) => schedule_report(
                SCHEDULE_STATUS_UNAVAILABLE,
                files,
                DIAGNOSTICS_STATUS_UNAVAILABLE,
            ),
        }
    }
}

fn queue() -> Option<&'static SyncSender<LspWriteDiagnosticsJob>> {
    static QUEUE: OnceLock<Option<SyncSender<LspWriteDiagnosticsJob>>> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel(LSP_BACKGROUND_QUEUE_CAPACITY);
            thread::Builder::new()
                .name(LSP_WRITE_DIAGNOSTICS_WORKER_NAME.to_owned())
                .spawn(move || worker_loop(receiver))
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

fn wait_inline(
    receiver: Receiver<LspDeferredDiagnosticsReport>,
    files: Vec<String>,
    deferred_sink: Option<LspDeferredDiagnosticsSink>,
) -> LspWriteDiagnosticsSchedule {
    let inline_wait = Duration::from_millis(LSP_WRITE_DIAGNOSTICS_INLINE_WAIT_MS);
    match receiver.recv_timeout(inline_wait) {
        Ok(report) => schedule_from_report(files, report),
        Err(RecvTimeoutError::Timeout) => {
            schedule_deferred_collector(receiver, deferred_sink);
            schedule_report(
                SCHEDULE_STATUS_SCHEDULED,
                files,
                DIAGNOSTICS_STATUS_DEFERRED,
            )
        }
        Err(RecvTimeoutError::Disconnected) => schedule_report(
            SCHEDULE_STATUS_UNAVAILABLE,
            files,
            DIAGNOSTICS_STATUS_UNAVAILABLE,
        ),
    }
}

fn schedule_deferred_collector(
    receiver: Receiver<LspDeferredDiagnosticsReport>,
    deferred_sink: Option<LspDeferredDiagnosticsSink>,
) {
    let Some(deferred_sink) = deferred_sink else {
        return;
    };
    let deferred_wait = Duration::from_millis(
        LSP_WRITE_DIAGNOSTICS_DEFERRED_WAIT_MS.saturating_sub(LSP_WRITE_DIAGNOSTICS_INLINE_WAIT_MS),
    );
    let _ = thread::Builder::new()
        .name(LSP_WRITE_DIAGNOSTICS_COLLECTOR_NAME.to_owned())
        .spawn(move || {
            if let Ok(report) = receiver.recv_timeout(deferred_wait)
                && report.has_diagnostics()
            {
                deferred_sink(report);
            }
        });
}

fn schedule_report(
    status: &'static str,
    files: Vec<String>,
    diagnostics_status: &'static str,
) -> LspWriteDiagnosticsSchedule {
    LspWriteDiagnosticsSchedule {
        status,
        files,
        coalesced: 0,
        evicted: 0,
        diagnostics_status,
        documents: Vec::new(),
        failures: Vec::new(),
        stale: 0,
    }
}

fn schedule_from_report(
    files: Vec<String>,
    report: LspDeferredDiagnosticsReport,
) -> LspWriteDiagnosticsSchedule {
    LspWriteDiagnosticsSchedule {
        status: SCHEDULE_STATUS_SCHEDULED,
        files,
        coalesced: 0,
        evicted: 0,
        diagnostics_status: report.status,
        documents: report.documents,
        failures: report.failures,
        stale: report.stale,
    }
}

fn worker_loop(receiver: Receiver<LspWriteDiagnosticsJob>) {
    for job in receiver {
        let report = execute_job(&job.workspace_root, &job.changes, job.deadline);
        let _ = job.completion.send(report);
    }
}

fn execute_job(
    workspace_root: &Path,
    changes: &[NormalizedWriteChange],
    deadline: Instant,
) -> LspDeferredDiagnosticsReport {
    let mut documents = Vec::new();
    let mut failures = Vec::new();
    let mut stale = 0_usize;
    for change in changes {
        if deadline.saturating_duration_since(Instant::now()).is_zero() {
            failures.push("写后 diagnostics 超过延迟窗口".to_owned());
            break;
        }
        if !change_is_current(change) {
            stale = stale.saturating_add(1);
            continue;
        }
        let result = process_change(workspace_root, change, deadline);
        record_change_result(change, result, &mut documents, &mut failures, &mut stale);
    }
    let status = if failures.is_empty() {
        DIAGNOSTICS_STATUS_READY
    } else if documents.is_empty() {
        DIAGNOSTICS_STATUS_UNAVAILABLE
    } else {
        DIAGNOSTICS_STATUS_PARTIAL
    };
    LspDeferredDiagnosticsReport {
        status,
        documents,
        failures,
        stale,
    }
}

fn record_change_result(
    change: &NormalizedWriteChange,
    result: Result<Option<LspWriteDiagnosticsDocument>, String>,
    documents: &mut Vec<LspWriteDiagnosticsDocument>,
    failures: &mut Vec<String>,
    stale: &mut usize,
) {
    if !change_is_current(change) {
        *stale = stale.saturating_add(1);
        return;
    }
    match result {
        Ok(Some(document)) => documents.push(document),
        Ok(None) => {}
        Err(error) => failures.push(error),
    }
}

fn process_change(
    workspace_root: &Path,
    change: &NormalizedWriteChange,
    deadline: Instant,
) -> Result<Option<LspWriteDiagnosticsDocument>, String> {
    let uri = path_to_file_uri(&change.absolute_path)
        .map_err(|error| format!("{}: {error:?}", change.workspace_path))?;
    let plans = LspCatalogRuntime::resolve_document_launch_plans(workspace_root, &uri)
        .map_err(|error| format!("{}: {error:?}", change.workspace_path))?;
    if plans.is_empty() {
        return Ok(None);
    }
    let protocol_change = LspWorkspaceFileChange {
        uri: uri.clone(),
        kind: change.kind.protocol_kind(),
    };
    let mut servers = Vec::new();
    let mut diagnostics = Vec::new();
    let mut failures = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        if deadline.saturating_duration_since(Instant::now()).is_zero() {
            failures.push("写后 diagnostics 超过延迟窗口".to_owned());
            break;
        }
        let response = LspClientPoolRuntime::with_client_until(plan, deadline, |client| {
            client.notify_workspace_file_changes(std::slice::from_ref(&protocol_change))?;
            if change.kind == LspWriteFileChangeKind::Deleted {
                return Ok(None);
            }
            let document_changed = client.sync_and_notify_document_saved(&uri)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(axon_domain::AxonError::tool_invalid_definition(
                    "写后 diagnostics 超过延迟窗口",
                ));
            }
            client
                .document_diagnostic_after_save_with_timeout(
                    &LspDiagnosticRequest {
                        id: u64::try_from(index.saturating_add(10_000)).unwrap_or(10_000),
                        text_document_uri: uri.clone(),
                    },
                    remaining,
                    document_changed,
                )
                .map(Some)
        });
        match response {
            Ok(response) => {
                servers.push(plan.server_id.clone());
                if let Some(response) = response {
                    diagnostics.extend(response_diagnostics(&plan.server_id, &response));
                }
            }
            Err(error) => failures.push(format!("{}: {error:?}", plan.server_id)),
        }
    }
    if servers.is_empty() && !failures.is_empty() {
        return Err(format!(
            "{} LSP diagnostics 全部失败：{}",
            change.workspace_path,
            failures.join("; ")
        ));
    }
    if change.kind == LspWriteFileChangeKind::Deleted {
        return Ok(None);
    }
    Ok(Some(LspWriteDiagnosticsDocument {
        workspace_path: change.workspace_path.clone(),
        servers,
        diagnostics,
    }))
}

fn response_diagnostics(server_id: &str, response: &Value) -> Vec<Value> {
    response
        .get("result")
        .and_then(|result| result.get("items"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .cloned()
        .map(|mut diagnostic| {
            if let Some(object) = diagnostic.as_object_mut() {
                object
                    .entry("serverId".to_owned())
                    .or_insert_with(|| json!(server_id));
            }
            diagnostic
        })
        .collect()
}

fn normalize_changes(
    workspace_root: &Path,
    changes: impl IntoIterator<Item = LspWriteFileChange>,
) -> Vec<NormalizedWriteChange> {
    let mut normalized = Vec::<NormalizedWriteChange>::new();
    let mut positions = HashMap::<PathBuf, usize>::new();
    for change in changes {
        let absolute_path = if change.path.is_absolute() {
            change.path
        } else {
            workspace_root.join(change.path)
        };
        if !LspCatalogRuntime::supports_path(&absolute_path) {
            continue;
        }
        let absolute_path = absolute_path.canonicalize().unwrap_or(absolute_path);
        if change.kind != LspWriteFileChangeKind::Deleted && !absolute_path.is_file() {
            continue;
        }
        let workspace_path = absolute_path
            .strip_prefix(workspace_root)
            .unwrap_or(&absolute_path)
            .display()
            .to_string();
        let expected_sha256 = if change.kind == LspWriteFileChangeKind::Deleted {
            None
        } else {
            std::fs::read(&absolute_path)
                .ok()
                .map(|content| sha256_hex(&content))
        };
        let normalized_change = NormalizedWriteChange {
            workspace_path,
            absolute_path: absolute_path.clone(),
            kind: change.kind,
            expected_sha256,
        };
        if let Some(index) = positions.get(&absolute_path).copied() {
            normalized[index] = normalized_change;
        } else {
            positions.insert(absolute_path, normalized.len());
            normalized.push(normalized_change);
        }
    }
    normalized
}

fn change_is_current(change: &NormalizedWriteChange) -> bool {
    match change.kind {
        LspWriteFileChangeKind::Deleted => !change.absolute_path.is_file(),
        LspWriteFileChangeKind::Created | LspWriteFileChangeKind::Changed => {
            change.expected_sha256.as_ref().is_some_and(|expected| {
                std::fs::read(&change.absolute_path)
                    .ok()
                    .is_some_and(|content| sha256_hex(&content) == *expected)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_changes_preserves_move_event_order_and_filters_unknown_language() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-write-move-order");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("new.rs"), "fn moved() {}\n").unwrap();
        let workspace = workspace.canonicalize().unwrap();

        let changes = normalize_changes(
            &workspace,
            [
                LspWriteFileChange {
                    path: PathBuf::from("old.rs"),
                    kind: LspWriteFileChangeKind::Deleted,
                },
                LspWriteFileChange {
                    path: PathBuf::from("ignored.unknown"),
                    kind: LspWriteFileChangeKind::Deleted,
                },
                LspWriteFileChange {
                    path: PathBuf::from("new.rs"),
                    kind: LspWriteFileChangeKind::Created,
                },
            ],
        );

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].workspace_path, "old.rs");
        assert_eq!(changes[0].kind, LspWriteFileChangeKind::Deleted);
        assert_eq!(changes[1].workspace_path, "new.rs");
        assert_eq!(changes[1].kind, LspWriteFileChangeKind::Created);
    }

    #[test]
    fn stale_changed_file_is_rejected_by_content_digest() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-write-stale");
        std::fs::create_dir_all(&workspace).unwrap();
        let file = workspace.join("main.rs");
        std::fs::write(&file, "fn first() {}\n").unwrap();
        let change = normalize_changes(
            &workspace,
            [LspWriteFileChange {
                path: file.clone(),
                kind: LspWriteFileChangeKind::Changed,
            }],
        )
        .pop()
        .unwrap();

        std::fs::write(file, "fn second() {}\n").unwrap();

        assert!(!change_is_current(&change));
    }

    #[test]
    fn result_is_discarded_when_file_changes_during_diagnostics() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-write-result-stale");
        std::fs::create_dir_all(&workspace).unwrap();
        let file = workspace.join("main.rs");
        std::fs::write(&file, "fn first() {}\n").unwrap();
        let change = normalize_changes(
            &workspace,
            [LspWriteFileChange {
                path: file.clone(),
                kind: LspWriteFileChangeKind::Changed,
            }],
        )
        .pop()
        .unwrap();
        let result = Ok(Some(LspWriteDiagnosticsDocument {
            workspace_path: "main.rs".to_owned(),
            servers: vec!["rust-analyzer".to_owned()],
            diagnostics: vec![json!({ "message": "stale" })],
        }));
        std::fs::write(file, "fn second() {}\n").unwrap();
        let mut documents = Vec::new();
        let mut failures = Vec::new();
        let mut stale = 0;

        record_change_result(&change, result, &mut documents, &mut failures, &mut stale);

        assert!(documents.is_empty());
        assert!(failures.is_empty());
        assert_eq!(stale, 1);
    }

    #[test]
    fn timeout_returns_deferred_and_late_report_reaches_sink() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (sink_sender, sink_receiver) = mpsc::sync_channel(1);
        let sink: LspDeferredDiagnosticsSink = Arc::new(move |report| {
            let _ = sink_sender.send(report);
        });
        let report = LspDeferredDiagnosticsReport {
            status: DIAGNOSTICS_STATUS_READY,
            documents: vec![LspWriteDiagnosticsDocument {
                workspace_path: "src/main.rs".to_owned(),
                servers: vec!["rust-analyzer".to_owned()],
                diagnostics: vec![json!({ "message": "late" })],
            }],
            failures: Vec::new(),
            stale: 0,
        };

        let schedule = wait_inline(receiver, vec!["src/main.rs".to_owned()], Some(sink));
        let _ = sender.send(report);

        assert_eq!(schedule.diagnostics_status, DIAGNOSTICS_STATUS_DEFERRED);
        assert_eq!(
            sink_receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .documents[0]
                .diagnostics[0]["message"],
            "late"
        );
    }
}
