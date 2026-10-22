use std::io::BufRead;

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::lsp_document::file_path_from_uri;

pub(super) fn workspace_name(root_uri: &str) -> String {
    file_path_from_uri(root_uri)
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "workspace".to_owned())
}

pub(super) fn read_message_from(reader: &mut impl BufRead) -> AxonResult<Value> {
    let content_length = read_content_length(reader)?;
    let mut body = vec![0_u8; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|_| AxonError::tool_invalid_definition("LSP 响应读取失败"))?;
    serde_json::from_slice::<Value>(&body)
        .map_err(|_| AxonError::tool_invalid_definition("LSP 响应不是有效 JSON"))
}

pub(super) fn read_content_length(reader: &mut impl BufRead) -> AxonResult<usize> {
    let mut line = String::new();
    let mut content_length = None;
    loop {
        line.clear();
        reader
            .read_line(&mut line)
            .map_err(|_| AxonError::tool_invalid_definition("LSP 头部读取失败"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    content_length.ok_or(AxonError::tool_invalid_definition(
        "LSP 响应缺少 Content-Length",
    ))
}
