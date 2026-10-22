//! MCP 工具输出投影。

use std::collections::BTreeMap;

use serde_json::{Map, Value};

const MCP_RESULT_CONTENT_BLOCKS_KEY: &str = "contentBlocks";
const MCP_RESULT_OUTPUT_PREVIEW_KEY: &str = "outputPreview";
const MCP_RESULT_CONTENT_KEY: &str = "content";
const MCP_RESULT_CONTENT_TYPE_KEY: &str = "type";
const MCP_RESULT_CONTENT_TEXT_KEY: &str = "text";
const MCP_RESULT_CONTENT_KIND_TEXT: &str = "text";
const MCP_RESULT_CONTENT_KIND_IMAGE: &str = "image";
const MCP_RESULT_CONTENT_KIND_AUDIO: &str = "audio";
const MCP_RESULT_CONTENT_KIND_RESOURCE: &str = "resource";
const MCP_RESULT_CONTENT_KIND_RESOURCE_LINK: &str = "resource_link";
const MCP_RESULT_CONTENT_KIND_OTHER: &str = "other";
const MCP_RESULT_CONTENT_BLOCK_KIND_KEY: &str = "kind";
const MCP_RESULT_CONTENT_RESOURCE_KEY: &str = "resource";
const MCP_RESULT_CONTENT_URI_KEY: &str = "uri";
const MCP_RESULT_CONTENT_NAME_KEY: &str = "name";
const MCP_RESULT_CONTENT_MIME_TYPE_KEY: &str = "mimeType";
const MCP_RESULT_EMPTY_SUMMARY: &str = "MCP 返回空结果";
const MCP_RESULT_ADDITIONAL_CONTENT_PREFIX: &str = "附加结果：";
const MCP_RESULT_CONTENT_SUMMARY_PREFIX: &str = "MCP 返回：";
const MCP_RESULT_FILE_CHANGES_KEY: &str = "files";
const MCP_RESULT_FILE_CHANGE_KEYS: [&str; 3] = ["files", "fileChanges", "changes"];
const MCP_RESULT_FILE_CHANGE_CONTAINERS: [&str; 3] = ["structuredContent", "data", "payload"];

/// Runtime MCP 工具调用输出。
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeMcpToolCallOutput {
    /// 模型可见摘要。
    pub summary: String,
    /// 脱敏结构化 metadata。
    pub metadata: serde_json::Value,
    /// 是否执行了文件系统写入。
    pub filesystem_write_performed: bool,
}

pub(crate) fn runtime_mcp_http_output(
    server: &str,
    tool: &str,
    mcp_tool: &str,
    report: &axon_agent_runtime::McpStreamableHttpCallReport,
) -> RuntimeMcpToolCallOutput {
    let result = mcp_result_value(&report.result_text);
    let summary = mcp_result_preview(&result);
    let projection = MappedMcpResultProjection {
        is_error: report.result_projection.is_error,
        content_types: report.result_projection.content_types.clone(),
        structured_content_declared: report.result_projection.structured_content_declared,
        structured_content_keys: report.result_projection.structured_content_keys.clone(),
    };
    runtime_mcp_output(RuntimeMcpOutputInput {
        server,
        tool,
        mcp_tool,
        transport: "streamable_http",
        summary,
        result,
        projection,
        notification_summary: Some(&report.notification_summary),
    })
}

pub(crate) fn runtime_mcp_stdio_output(
    server: &str,
    tool: &str,
    mcp_tool: &str,
    report: &axon_agent_runtime::McpPersistentCallResult,
) -> RuntimeMcpToolCallOutput {
    let result = mcp_result_value(&report.result_text);
    let summary = mcp_result_preview(&result);
    let projection = MappedMcpResultProjection {
        is_error: report.result_projection.is_error,
        content_types: report.result_projection.content_types.clone(),
        structured_content_declared: report.result_projection.structured_content_declared,
        structured_content_keys: report.result_projection.structured_content_keys.clone(),
    };
    runtime_mcp_output(RuntimeMcpOutputInput {
        server,
        tool,
        mcp_tool,
        transport: "stdio",
        summary,
        result,
        projection,
        notification_summary: None,
    })
}

struct RuntimeMcpOutputInput<'a> {
    server: &'a str,
    tool: &'a str,
    mcp_tool: &'a str,
    transport: &'a str,
    summary: String,
    result: Value,
    projection: MappedMcpResultProjection,
    notification_summary: Option<&'a axon_agent_runtime::McpNotificationSummary>,
}

struct MappedMcpResultProjection {
    is_error: bool,
    content_types: Vec<String>,
    structured_content_declared: bool,
    structured_content_keys: Vec<String>,
}

fn runtime_mcp_output(input: RuntimeMcpOutputInput<'_>) -> RuntimeMcpToolCallOutput {
    let structured_content = input
        .result
        .get("structuredContent")
        .cloned()
        .unwrap_or(Value::Null);
    let content_blocks = project_mcp_content_blocks(&input.result);
    let mut metadata = serde_json::json!({
        "kind": "mcp",
        "server": input.server,
        "tool": input.tool,
        "mcpTool": input.mcp_tool,
        "transport": input.transport,
        "result": {
            "isError": input.projection.is_error,
            "contentTypes": input.projection.content_types,
            "structuredContentDeclared": input.projection.structured_content_declared,
            "structuredContentKeys": input.projection.structured_content_keys,
        },
        MCP_RESULT_CONTENT_BLOCKS_KEY: content_blocks,
        "structuredContent": structured_content,
        MCP_RESULT_OUTPUT_PREVIEW_KEY: input.summary.clone(),
        "rawPayloadExposed": false,
    });
    if let Some(summary) = input.notification_summary {
        metadata["notificationSummary"] = serde_json::json!({
            "progressCount": summary.progress_count,
            "logMessageCount": summary.log_message_count,
            "cancelledCount": summary.cancelled_count,
            "serverCancelled": summary.server_cancelled,
            "rawPayloadExposed": summary.raw_payload_exposed,
        });
    }
    project_structured_file_changes(&mut metadata, &input.result);
    let filesystem_write_performed = metadata_has_file_changes(&metadata);
    RuntimeMcpToolCallOutput {
        summary: input.summary,
        metadata,
        filesystem_write_performed,
    }
}

fn mcp_result_value(text: &str) -> Value {
    let parsed = serde_json::from_str::<Value>(text).unwrap_or(Value::Null);
    parsed.get("result").cloned().unwrap_or(parsed)
}

fn mcp_result_summary(result: &Value) -> Option<String> {
    result
        .get(MCP_RESULT_CONTENT_KEY)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|content| mcp_content_kind(content) == MCP_RESULT_CONTENT_KIND_TEXT)
        .filter_map(|content| {
            content
                .get(MCP_RESULT_CONTENT_TEXT_KEY)
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(str::to_owned)
}

fn mcp_result_preview(result: &Value) -> String {
    let text = mcp_result_summary(result);
    let content_summary = mcp_content_summary(result, text.is_some());
    match (text, content_summary) {
        (Some(text), Some(content_summary)) => {
            format!("{text}\n{MCP_RESULT_ADDITIONAL_CONTENT_PREFIX}{content_summary}")
        }
        (Some(text), None) => text,
        (None, Some(content_summary)) => {
            format!("{MCP_RESULT_CONTENT_SUMMARY_PREFIX}{content_summary}")
        }
        (None, None) => MCP_RESULT_EMPTY_SUMMARY.to_owned(),
    }
}

fn mcp_content_summary(result: &Value, exclude_text: bool) -> Option<String> {
    let mut counts = BTreeMap::<&str, u64>::new();
    for content in mcp_content_values(result) {
        let kind = mcp_content_kind(content);
        if exclude_text && kind == MCP_RESULT_CONTENT_KIND_TEXT {
            continue;
        }
        *counts.entry(kind).or_default() += 1;
    }
    let summary = [
        MCP_RESULT_CONTENT_KIND_TEXT,
        MCP_RESULT_CONTENT_KIND_IMAGE,
        MCP_RESULT_CONTENT_KIND_AUDIO,
        MCP_RESULT_CONTENT_KIND_RESOURCE,
        MCP_RESULT_CONTENT_KIND_RESOURCE_LINK,
        MCP_RESULT_CONTENT_KIND_OTHER,
    ]
    .into_iter()
    .filter_map(|kind| {
        counts
            .get(kind)
            .map(|count| format!("{} {count} 个", mcp_content_kind_label(kind)))
    })
    .collect::<Vec<_>>();
    (!summary.is_empty()).then(|| summary.join("、"))
}

fn mcp_content_kind_label(kind: &str) -> &'static str {
    match kind {
        MCP_RESULT_CONTENT_KIND_TEXT => "文本",
        MCP_RESULT_CONTENT_KIND_IMAGE => "图片",
        MCP_RESULT_CONTENT_KIND_AUDIO => "音频",
        MCP_RESULT_CONTENT_KIND_RESOURCE => "资源",
        MCP_RESULT_CONTENT_KIND_RESOURCE_LINK => "资源链接",
        _ => "其他内容",
    }
}

fn project_mcp_content_blocks(result: &Value) -> Value {
    Value::Array(
        mcp_content_values(result)
            .map(project_mcp_content_block)
            .collect(),
    )
}

fn mcp_content_values(result: &Value) -> impl Iterator<Item = &Value> {
    result
        .get(MCP_RESULT_CONTENT_KEY)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn project_mcp_content_block(content: &Value) -> Value {
    let kind = mcp_content_kind(content);
    let mut projection = Map::new();
    projection.insert(
        MCP_RESULT_CONTENT_BLOCK_KIND_KEY.to_owned(),
        Value::String(kind.to_owned()),
    );
    match kind {
        MCP_RESULT_CONTENT_KIND_TEXT => {
            if let Some(text) = content
                .get(MCP_RESULT_CONTENT_TEXT_KEY)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                projection.insert(
                    MCP_RESULT_CONTENT_TEXT_KEY.to_owned(),
                    Value::String(text.to_owned()),
                );
            }
        }
        MCP_RESULT_CONTENT_KIND_RESOURCE => {
            if let Some(resource) = content.get(MCP_RESULT_CONTENT_RESOURCE_KEY) {
                project_content_reference_fields(&mut projection, resource);
            }
        }
        MCP_RESULT_CONTENT_KIND_IMAGE
        | MCP_RESULT_CONTENT_KIND_AUDIO
        | MCP_RESULT_CONTENT_KIND_RESOURCE_LINK => {
            project_content_reference_fields(&mut projection, content);
        }
        _ => {}
    }
    Value::Object(projection)
}

fn mcp_content_kind(content: &Value) -> &'static str {
    match content
        .get(MCP_RESULT_CONTENT_TYPE_KEY)
        .and_then(Value::as_str)
    {
        Some(MCP_RESULT_CONTENT_KIND_TEXT) => MCP_RESULT_CONTENT_KIND_TEXT,
        Some(MCP_RESULT_CONTENT_KIND_IMAGE) => MCP_RESULT_CONTENT_KIND_IMAGE,
        Some(MCP_RESULT_CONTENT_KIND_AUDIO) => MCP_RESULT_CONTENT_KIND_AUDIO,
        Some(MCP_RESULT_CONTENT_KIND_RESOURCE) => MCP_RESULT_CONTENT_KIND_RESOURCE,
        Some(MCP_RESULT_CONTENT_KIND_RESOURCE_LINK) => MCP_RESULT_CONTENT_KIND_RESOURCE_LINK,
        _ => MCP_RESULT_CONTENT_KIND_OTHER,
    }
}

fn project_content_reference_fields(projection: &mut Map<String, Value>, source: &Value) {
    for key in [
        MCP_RESULT_CONTENT_URI_KEY,
        MCP_RESULT_CONTENT_NAME_KEY,
        MCP_RESULT_CONTENT_MIME_TYPE_KEY,
    ] {
        if let Some(value) = source
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            projection.insert(key.to_owned(), Value::String(value.to_owned()));
        }
    }
}

fn project_structured_file_changes(metadata: &mut Value, result: &Value) {
    let Some(files) = result_file_changes(result) else {
        return;
    };
    let Some(object) = metadata.as_object_mut() else {
        return;
    };
    object.insert(MCP_RESULT_FILE_CHANGES_KEY.to_owned(), files);
}

fn result_file_changes(result: &Value) -> Option<Value> {
    file_changes_value(result).or_else(|| {
        MCP_RESULT_FILE_CHANGE_CONTAINERS
            .iter()
            .find_map(|key| result.get(*key).and_then(file_changes_value))
    })
}

fn file_changes_value(value: &Value) -> Option<Value> {
    MCP_RESULT_FILE_CHANGE_KEYS.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty() && items.iter().all(file_change_has_path))
            .cloned()
            .map(Value::Array)
    })
}

fn file_change_has_path(value: &Value) -> bool {
    ["path", "file", "filePath", "newPath", "to", "target"]
        .iter()
        .any(|key| value.get(*key).and_then(Value::as_str).is_some())
}

fn metadata_has_file_changes(metadata: &Value) -> bool {
    metadata
        .get(MCP_RESULT_FILE_CHANGES_KEY)
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_output_projects_top_level_file_changes_to_diff_contract() {
        let report = persistent_call_result(&serde_json::json!({
            "content": [{"type": "text", "text": "edited src/lib.rs"}],
            "fileChanges": [{
                "path": "src/lib.rs",
                "operation": "updated",
                "diff": "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n"
            }]
        }));

        let output = runtime_mcp_stdio_output("fs", "edit", "edit", &report);

        assert!(output.filesystem_write_performed);
        assert_eq!(output.metadata["outputPreview"], "edited src/lib.rs");
        assert_eq!(output.metadata["rawPayloadExposed"], false);
        assert_eq!(output.metadata["files"][0]["path"], "src/lib.rs");
        assert_eq!(output.metadata["files"][0]["operation"], "updated");
        assert!(output.metadata["fileChanges"].is_null());
    }

    #[test]
    fn mcp_output_projects_nested_payload_changes_to_diff_contract() {
        let report = persistent_call_result(&serde_json::json!({
            "content": [{"type": "text", "text": "created src/new.rs"}],
            "payload": {
                "changes": [{
                    "filePath": "src/new.rs",
                    "type": "added",
                    "diff": "pub fn new() {}\n"
                }]
            }
        }));

        let output = runtime_mcp_stdio_output("fs", "create", "create", &report);

        assert!(output.filesystem_write_performed);
        assert_eq!(output.metadata["files"][0]["filePath"], "src/new.rs");
        assert_eq!(output.metadata["files"][0]["type"], "added");
    }

    #[test]
    fn mcp_output_preserves_long_text_and_all_content_blocks() {
        let long_text = "x".repeat(9_000);
        let mut content = vec![serde_json::json!({"type": "text", "text": long_text})];
        content.extend((0..20).map(|index| {
            serde_json::json!({
                "type": "resource_link",
                "uri": format!("file:///tmp/{index}.json"),
                "name": format!("resource-{index}"),
                "mimeType": "application/json"
            })
        }));
        let report = persistent_call_result(&serde_json::json!({"content": content}));

        let output = runtime_mcp_stdio_output("search", "inspect", "inspect", &report);

        assert_eq!(
            output.summary.lines().next().unwrap().chars().count(),
            9_000
        );
        assert_eq!(
            output.metadata[MCP_RESULT_CONTENT_BLOCKS_KEY]
                .as_array()
                .unwrap()
                .len(),
            21
        );
        assert_eq!(
            output.metadata[MCP_RESULT_CONTENT_BLOCKS_KEY][20]["uri"],
            "file:///tmp/19.json"
        );
    }

    #[test]
    fn mcp_output_projects_mixed_content_without_binary_payloads() {
        let report = persistent_call_result(&serde_json::json!({
            "content": [
                {"type": "text", "text": "已找到匹配项"},
                {"type": "image", "data": "base64-image", "mimeType": "image/png"},
                {"type": "resource_link", "uri": "file:///tmp/report.json", "name": "报告", "mimeType": "application/json"},
                {"type": "audio", "data": "base64-audio", "mimeType": "audio/mpeg"}
            ]
        }));

        let output = runtime_mcp_stdio_output("search", "inspect", "inspect", &report);

        assert_eq!(
            output.summary,
            "已找到匹配项\n附加结果：图片 1 个、音频 1 个、资源链接 1 个"
        );
        assert_eq!(
            output.metadata[MCP_RESULT_CONTENT_BLOCKS_KEY][1]["mimeType"],
            "image/png"
        );
        assert_eq!(
            output.metadata[MCP_RESULT_CONTENT_BLOCKS_KEY][2]["uri"],
            "file:///tmp/report.json"
        );
        assert!(!output.metadata.to_string().contains("base64-image"));
        assert!(!output.metadata.to_string().contains("base64-audio"));
        assert_eq!(output.metadata["rawPayloadExposed"], false);
    }

    #[test]
    fn mcp_output_uses_safe_summary_when_no_text_content_exists() {
        let report = persistent_call_result(&serde_json::json!({
            "content": [{"type": "image", "data": "base64-image", "mimeType": "image/png"}]
        }));

        let output = runtime_mcp_stdio_output("vision", "capture", "capture", &report);

        assert_eq!(output.summary, "MCP 返回：图片 1 个");
        assert_eq!(
            output.metadata[MCP_RESULT_OUTPUT_PREVIEW_KEY],
            "MCP 返回：图片 1 个"
        );
        assert!(!output.metadata.to_string().contains("base64-image"));
    }

    fn persistent_call_result(result: &Value) -> axon_agent_runtime::McpPersistentCallResult {
        axon_agent_runtime::McpPersistentCallResult {
            request_id: 100,
            result_text: result.to_string(),
            result_projection: axon_agent_runtime::McpToolCallResultProjection {
                is_error: false,
                content_types: vec!["text".to_owned()],
                structured_content_declared: false,
                structured_content_keys: Vec::new(),
                raw_payload_exposed: false,
            },
            reasons: vec!["test".to_owned()],
        }
    }
}
