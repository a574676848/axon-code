//! 人设与行为文档 HTTP 适配器。
//!
//! 两条文档都在运行时根下，文件名与桌面端 `documents.rs` 完全一致：
//! `SOUL.md` 与 `AGENTS.md`。载荷形状固定为 `{"text": "<全文>"}`，
//! 文件不存在时回空串而**不是** 404：首次安装还没有这两份文件，界面据此
//! 渲染空编辑器，而不是弹一个错误。
//!
//! 写入走 `axon_config` 的原子替换原语，与桌面端文档写入同一策略。

use std::path::{Path, PathBuf};

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde_json::{Value, json};

/// 人设文档路由。
const SOUL_PATH: &str = "/api/v1/documents/soul";
/// 行为文档路由。
const AGENTS_PATH: &str = "/api/v1/documents/agents";
/// 人设文件名。
const SOUL_FILE: &str = "SOUL.md";
/// 行为约束文件名。
const AGENTS_FILE: &str = "AGENTS.md";
/// 方法不匹配的错误码，与网关自身的 405 口径同名。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 请求体不是合法 JSON 的错误码。
const INVALID_JSON: &str = "invalid_json";
/// 缺少 `text` 字段的错误码。
const MISSING_TEXT: &str = "missing_text";
/// 正文为空的错误码。
const EMPTY_TEXT: &str = "empty_text";
/// 读取失败的错误码。
const READ_FAILED: &str = "read_failed";
/// 写入失败的错误码。
const WRITE_FAILED: &str = "write_failed";

/// 人设与行为文档适配器。
pub(crate) struct DocumentsHttpAdapter {
    /// Axon 运行时根目录；两份文档都位于其下。
    axon_root: PathBuf,
}

impl DocumentsHttpAdapter {
    /// 以运行时根目录构造。
    pub(crate) fn new(axon_root: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
        }
    }

    /// 读取一份文档：文件不存在时回空串。
    fn read(&self, file_name: &str) -> GatewayHttpResponse {
        let path = self.axon_root.join(file_name);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                // BOM 不属于正文，回给编辑器的必须是干净文本。
                let normalized = text.trim_start_matches('\u{feff}');
                GatewayHttpResponse::json(200, &json!({ "text": normalized }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                GatewayHttpResponse::json(200, &json!({ "text": "" }))
            }
            Err(error) => GatewayHttpResponse::json(
                500,
                &json!({ "error": READ_FAILED, "message": format!("读取文档失败：{error}") }),
            ),
        }
    }

    /// 写入一份文档：先校验请求体，再原子替换。
    fn write(&self, file_name: &str, body: &[u8]) -> GatewayHttpResponse {
        let Ok(request) = serde_json::from_slice::<Value>(body) else {
            return GatewayHttpResponse::json(400, &json!({ "error": INVALID_JSON }));
        };
        let Some(text) = request.get("text").and_then(Value::as_str) else {
            return GatewayHttpResponse::json(400, &json!({ "error": MISSING_TEXT }));
        };
        // 与桌面端同一口径：两份文档是运行态输入，写成空文件等于把人设或行为约束抹掉。
        if text.trim().is_empty() {
            return GatewayHttpResponse::json(
                400,
                &json!({ "error": EMPTY_TEXT, "message": "文档内容不能为空" }),
            );
        }
        if let Err(response) = write_document_text(&self.axon_root.join(file_name), text) {
            return response;
        }
        GatewayHttpResponse::json(200, &json!({ "text": text }))
    }
}

/// 原子写入文本，父目录按需创建。
fn write_document_text(path: &Path, text: &str) -> Result<(), GatewayHttpResponse> {
    axon_config::atomic_write::replace_file_atomically(path, text.as_bytes()).map_err(|error| {
        GatewayHttpResponse::json(
            500,
            &json!({ "error": WRITE_FAILED, "message": format!("原子写入失败：{error:?}") }),
        )
    })
}

impl GatewayHttpRouteHandler for DocumentsHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        let file_name = if path == SOUL_PATH {
            SOUL_FILE
        } else if path == AGENTS_PATH {
            AGENTS_FILE
        } else {
            // 未命中一律交回组合器，不得返回统一 404 截断后续适配器。
            return None;
        };
        Some(match method {
            "GET" => self.read(file_name),
            "PUT" => self.write(file_name, body),
            _ => GatewayHttpResponse::json(405, &json!({ "error": METHOD_NOT_ALLOWED })),
        })
    }
}

#[cfg(test)]
#[path = "documents_http_adapter_tests.rs"]
mod documents_http_adapter_tests;
