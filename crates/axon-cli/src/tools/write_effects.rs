//! 文件写入后的会话快照、watch/cache 失效与 LSP 刷新协调。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axon_agent_runtime::{
    AgentRuntimeNoteKind, LspDeferredDiagnosticsSink, LspWriteDiagnosticsRuntime,
    LspWriteFileChange, LspWriteFileChangeKind,
};

use crate::tool_orchestrator::ToolOrchestratorContext;

/// 已提交文件变更的写后语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceWriteEffectKind {
    Created,
    Changed,
    Deleted,
}

/// 一个已提交文件变更中，模型实际可见的新文件侧行号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceWriteEffect {
    pub(crate) path: PathBuf,
    pub(crate) source_path: Option<PathBuf>,
    pub(crate) kind: WorkspaceWriteEffectKind,
    pub(crate) seen_lines: Vec<usize>,
}

impl WorkspaceWriteEffect {
    pub(crate) fn created(path: impl Into<PathBuf>, seen_lines: Vec<usize>) -> Self {
        Self::new(path, WorkspaceWriteEffectKind::Created, seen_lines)
    }

    pub(crate) fn changed(path: impl Into<PathBuf>, seen_lines: Vec<usize>) -> Self {
        Self::new(path, WorkspaceWriteEffectKind::Changed, seen_lines)
    }

    pub(crate) fn deleted(path: impl Into<PathBuf>) -> Self {
        Self::new(path, WorkspaceWriteEffectKind::Deleted, Vec::new())
    }

    fn new(
        path: impl Into<PathBuf>,
        kind: WorkspaceWriteEffectKind,
        seen_lines: Vec<usize>,
    ) -> Self {
        Self {
            path: path.into(),
            source_path: None,
            kind,
            seen_lines,
        }
    }

    pub(crate) fn moved(
        source_path: impl Into<PathBuf>,
        path: impl Into<PathBuf>,
        seen_lines: Vec<usize>,
    ) -> Self {
        Self {
            path: path.into(),
            source_path: Some(source_path.into()),
            kind: WorkspaceWriteEffectKind::Created,
            seen_lines,
        }
    }
}

/// 在写入事务成功后同步会话快照，并非阻塞调度全部变更文档的 LSP 触达。
pub(crate) fn reconcile_workspace_write(
    context: &ToolOrchestratorContext,
    effects: &[WorkspaceWriteEffect],
) -> serde_json::Value {
    let root = context
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| context.cwd.clone());
    let mut warnings = Vec::new();
    for effect in effects {
        if let Some(source_path) = effect.source_path.as_deref() {
            context.workspace_files.invalidate(&root.join(source_path));
        }
        reconcile_target(context, &root, effect, &mut warnings);
    }
    let changes = effects.iter().flat_map(lsp_changes).collect::<Vec<_>>();
    let session_id = context.thread_session_id.clone();
    let deferred_sink: LspDeferredDiagnosticsSink = Arc::new(move |report| {
        let payload = serde_json::to_string(&report)
            .unwrap_or_else(|_| "{\"status\":\"unavailable\"}".to_owned());
        let _ = crate::run_commands::run_steer::try_send_deferred_runtime_note(
            &session_id,
            AgentRuntimeNoteKind::DeferredDiagnostics,
            format!("edit 写后 LSP 延迟诊断已到达。结果仅对应仍保持本次写入内容的文件：{payload}"),
        );
    });
    let schedule = LspWriteDiagnosticsRuntime::schedule(&context.cwd, changes, Some(deferred_sink));
    let mut lsp = serde_json::to_value(schedule).unwrap_or_else(|_| {
        serde_json::json!({
            "status": "unavailable",
            "files": [],
        })
    });
    if !warnings.is_empty()
        && let Some(object) = lsp.as_object_mut()
    {
        object.insert(
            "workspaceStateWarnings".to_owned(),
            serde_json::json!(warnings),
        );
    }
    lsp
}

fn lsp_changes(effect: &WorkspaceWriteEffect) -> Vec<LspWriteFileChange> {
    let mut changes = Vec::with_capacity(if effect.source_path.is_some() { 2 } else { 1 });
    if let Some(source_path) = effect.source_path.as_ref() {
        changes.push(LspWriteFileChange {
            path: source_path.clone(),
            kind: LspWriteFileChangeKind::Deleted,
        });
    }
    let kind = match effect.kind {
        WorkspaceWriteEffectKind::Created => LspWriteFileChangeKind::Created,
        WorkspaceWriteEffectKind::Changed => LspWriteFileChangeKind::Changed,
        WorkspaceWriteEffectKind::Deleted => LspWriteFileChangeKind::Deleted,
    };
    changes.push(LspWriteFileChange {
        path: effect.path.clone(),
        kind,
    });
    changes
}

fn reconcile_target(
    context: &ToolOrchestratorContext,
    root: &Path,
    effect: &WorkspaceWriteEffect,
    warnings: &mut Vec<String>,
) {
    let target = root.join(&effect.path);
    match std::fs::read_to_string(&target) {
        Ok(file_content) => {
            let canonical = target.canonicalize().unwrap_or(target);
            context.workspace_files.record_read(
                canonical,
                file_content,
                effect.seen_lines.iter().copied(),
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            context.workspace_files.invalidate(&target);
        }
        Err(error) => {
            context.workspace_files.invalidate(&target);
            warnings.push(format!(
                "{} 写后快照读取失败：{error}",
                effect.path.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_effect_emits_deleted_lsp_change_for_missing_path() {
        let changes = lsp_changes(&WorkspaceWriteEffect::deleted("src/old.rs"));

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, PathBuf::from("src/old.rs"));
        assert_eq!(changes[0].kind, LspWriteFileChangeKind::Deleted);
    }

    #[test]
    fn moved_effect_emits_source_deleted_and_target_created() {
        let changes = lsp_changes(&WorkspaceWriteEffect::moved(
            "src/old.rs",
            "src/new.rs",
            vec![1],
        ));

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, PathBuf::from("src/old.rs"));
        assert_eq!(changes[0].kind, LspWriteFileChangeKind::Deleted);
        assert_eq!(changes[1].path, PathBuf::from("src/new.rs"));
        assert_eq!(changes[1].kind, LspWriteFileChangeKind::Created);
    }
}
