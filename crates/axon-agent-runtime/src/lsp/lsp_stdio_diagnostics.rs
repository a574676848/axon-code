//! LSP stdio client 的文档诊断与 push diagnostics 收敛逻辑。

use super::*;

impl LspStdioClient {
    /// 执行 `textDocument/diagnostic` 查询。
    pub fn document_diagnostic(&mut self, request: &LspDiagnosticRequest) -> AxonResult<Value> {
        self.document_diagnostic_with_timeout(request, LSP_RESPONSE_TIMEOUT)
    }

    /// 在调用方给定的总窗口内执行 `textDocument/diagnostic` 查询。
    pub(crate) fn document_diagnostic_with_timeout(
        &mut self,
        request: &LspDiagnosticRequest,
        timeout: Duration,
    ) -> AxonResult<Value> {
        let deadline = Instant::now() + timeout;
        let document_changed = self.touch_document(&request.text_document_uri)?;
        self.document_diagnostic_after_sync_until(request, deadline, document_changed)
    }

    /// 查询已完成 didOpen/didChange 与 didSave 的写后文档诊断。
    pub(crate) fn document_diagnostic_after_save_with_timeout(
        &mut self,
        request: &LspDiagnosticRequest,
        timeout: Duration,
        document_changed: bool,
    ) -> AxonResult<Value> {
        self.document_diagnostic_after_sync_until(
            request,
            Instant::now() + timeout,
            document_changed,
        )
    }

    fn document_diagnostic_after_sync_until(
        &mut self,
        request: &LspDiagnosticRequest,
        deadline: Instant,
        document_changed: bool,
    ) -> AxonResult<Value> {
        let document_path = canonical_file_path_from_uri(&request.text_document_uri)?;
        let expected_version = self
            .documents
            .get(&request.text_document_uri)
            .map(|state| state.version);
        if !self.supports_pull_diagnostics {
            if document_changed
                || !self.published_diagnostics_are_current(&document_path, expected_version)
            {
                self.wait_for_published_diagnostics_until(
                    &document_path,
                    expected_version,
                    deadline,
                );
            }
            return Ok(self
                .published_diagnostic_response(request.id, &document_path)
                .unwrap_or_else(|| empty_diagnostic_response(request.id)));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AxonError::tool_invalid_definition("LSP 响应超时"));
        }
        let response = self.send_request_with_timeout(
            &LspClientRequest {
                id: request.id,
                method: "textDocument/diagnostic".to_owned(),
                params: json!({
                    "textDocument": { "uri": request.text_document_uri },
                }),
            },
            remaining,
        )?;
        if response.get("error").is_none() {
            return Ok(response);
        }
        if document_changed
            || !self.published_diagnostics_are_current(&document_path, expected_version)
        {
            self.wait_for_published_diagnostics_until(&document_path, expected_version, deadline);
        }
        Ok(self
            .published_diagnostic_response(request.id, &document_path)
            .unwrap_or(response))
    }

    fn wait_for_published_diagnostics_until(
        &mut self,
        path: &Path,
        expected_version: Option<i32>,
        outer_deadline: Instant,
    ) {
        let deadline = (Instant::now() + LSP_PUSH_DIAGNOSTICS_WAIT).min(outer_deadline);
        let mut settle_deadline: Option<Instant> = None;
        loop {
            let next_deadline = settle_deadline.map_or(deadline, |settle| settle.min(deadline));
            let remaining = next_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Some(incoming) = self.incoming.as_ref() else {
                break;
            };
            let Ok(message) = incoming.recv_timeout(remaining) else {
                break;
            };
            let Ok(message) = message else {
                break;
            };
            match self.handle_incoming_message(&message) {
                Ok(Some(published_path))
                    if published_path == path
                        && self.published_diagnostics_are_current(path, expected_version) =>
                {
                    settle_deadline = Some(Instant::now() + LSP_PUSH_DIAGNOSTICS_SETTLE);
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    fn published_diagnostics_are_current(
        &self,
        path: &Path,
        expected_version: Option<i32>,
    ) -> bool {
        let Some(params) = self.published_diagnostics.get(path) else {
            return false;
        };
        let Some(expected_version) = expected_version else {
            return true;
        };
        params
            .get("version")
            .and_then(Value::as_i64)
            .and_then(|version| i32::try_from(version).ok())
            .is_none_or(|version| version >= expected_version)
    }

    fn published_diagnostic_response(&self, request_id: u64, path: &Path) -> Option<Value> {
        let params = self.published_diagnostics.get(path)?;
        Some(json!({
            "jsonrpc": LSP_JSONRPC_VERSION,
            "id": request_id,
            "result": {
                "kind": "full",
                "items": params
                    .get("diagnostics")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            },
        }))
    }
}

fn empty_diagnostic_response(request_id: u64) -> Value {
    json!({
        "jsonrpc": LSP_JSONRPC_VERSION,
        "id": request_id,
        "result": {
            "kind": "full",
            "items": [],
        },
    })
}
