//! LSP 文档同步与工作区文件变更通知。

use std::fs;

use axon_domain::{AxonError, AxonResult};
use serde_json::json;

use crate::language_registry::LanguageRegistry;
use crate::lsp_document::file_path_from_uri;

use super::{LspDocumentState, LspStdioClient};

/// LSP workspace 文件变更类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspWorkspaceFileChangeKind {
    /// 新建文件。
    Created,
    /// 修改文件。
    Changed,
    /// 删除文件。
    Deleted,
}

impl LspWorkspaceFileChangeKind {
    const fn protocol_value(self) -> u8 {
        match self {
            Self::Created => 1,
            Self::Changed => 2,
            Self::Deleted => 3,
        }
    }
}

/// 一个发送给 LSP server 的 workspace 文件变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspWorkspaceFileChange {
    /// 文件 URI。
    pub uri: String,
    /// 变更类型。
    pub kind: LspWorkspaceFileChangeKind,
}

impl LspStdioClient {
    /// 通知 server 工作区文件已创建、修改或删除。
    pub fn notify_workspace_file_changes(
        &mut self,
        changes: &[LspWorkspaceFileChange],
    ) -> AxonResult<()> {
        for change in changes
            .iter()
            .filter(|change| change.kind == LspWorkspaceFileChangeKind::Deleted)
        {
            self.close_document(&change.uri)?;
        }
        let changes = changes
            .iter()
            .map(|change| {
                json!({
                    "uri": change.uri,
                    "type": change.kind.protocol_value(),
                })
            })
            .collect::<Vec<_>>();
        self.send_notification(
            "workspace/didChangeWatchedFiles",
            &json!({ "changes": changes }),
        )
    }

    fn close_document(&mut self, text_document_uri: &str) -> AxonResult<()> {
        if self.documents.remove(text_document_uri).is_none() {
            return Ok(());
        }
        self.send_notification(
            "textDocument/didClose",
            &json!({
                "textDocument": { "uri": text_document_uri },
            }),
        )
    }

    /// 将本机文件同步到语言服务器；首次发送 didOpen，内容变化时发送 didChange。
    pub fn touch_document(&mut self, text_document_uri: &str) -> AxonResult<bool> {
        let path = file_path_from_uri(text_document_uri)?;
        let text = fs::read_to_string(&path)
            .map_err(|_| AxonError::tool_execution_failed("LSP 目标文档不可读取"))?;
        let language = LanguageRegistry::resolve(&path)
            .map_or("plaintext", |resolved| resolved.lsp_language_id());
        let (method, params) = match self.documents.get(text_document_uri) {
            Some(state) if state.text == text => return Ok(false),
            Some(state) => {
                let version = state.version.saturating_add(1);
                (
                    "textDocument/didChange",
                    json!({
                        "textDocument": {
                            "uri": text_document_uri,
                            "version": version,
                        },
                        "contentChanges": [{ "text": text }],
                    }),
                )
            }
            None => (
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": text_document_uri,
                        "languageId": language,
                        "version": 1,
                        "text": text,
                    },
                }),
            ),
        };
        let version = self
            .documents
            .get(text_document_uri)
            .map_or(1, |state| state.version.saturating_add(1));
        self.send_notification(method, &params)?;
        self.documents.insert(
            text_document_uri.to_owned(),
            LspDocumentState { text, version },
        );
        Ok(true)
    }

    /// 写入提交后同步最新内容并发送 didSave，返回本次是否发生 didOpen/didChange。
    pub(crate) fn sync_and_notify_document_saved(
        &mut self,
        text_document_uri: &str,
    ) -> AxonResult<bool> {
        let document_changed = self.touch_document(text_document_uri)?;
        self.send_notification(
            "textDocument/didSave",
            &json!({
                "textDocument": { "uri": text_document_uri },
            }),
        )?;
        Ok(document_changed)
    }
}
