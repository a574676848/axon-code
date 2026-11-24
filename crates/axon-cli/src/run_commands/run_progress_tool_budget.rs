//! 工具结果进入事件流前的预算裁定。
//!
//! 与 dsh 的 spill 策略同形：正文只保留头尾，超出部分落归档，事件里带归档路径；
//! 体积型 metadata 字段换成「计数 + 归档路径」。模型可见内容与界面事件读同一份
//! 裁剪结果，因此单条事件天然有界，传输层不需要任何额外限制。

use std::path::PathBuf;

use axon_agent_runtime::{
    CONTENT_ARCHIVE_RESULT_STREAM, ContentArchiveRuntime, TOOL_RESULT_MAX_INLINE_BYTES,
    retain_head_tail_bytes, tool_output_source_trace_from_dispatch_name,
    tool_result_omission_notice,
};
use serde_json::{Map, Value};

/// metadata 里的归档路径键名；与既有归档门同一口径。
const METADATA_ARCHIVE_PATH_KEY: &str = "archivePath";
/// metadata 单键保留上限：超过即按体积型字段处理。
const METADATA_SCALAR_KEEP_BYTES: usize = 2 * 1024;
/// 归档预览长度。
const ARCHIVE_PREVIEW_CHARS: usize = 200;

/// 工具结果预算：正文头尾保留 + 全文归档 + 体积型字段换计数。
#[derive(Clone, Default)]
pub(super) struct ToolResultBudget {
    archive_root: Option<PathBuf>,
}

impl ToolResultBudget {
    /// 绑定运行态归档根；未绑定时只做裁剪，不落归档。
    pub(super) fn with_archive_root(archive_root: PathBuf) -> Self {
        Self {
            archive_root: Some(archive_root),
        }
    }

    /// 流式分片：只裁剪不归档，完整正文由完成态那条事件负责归档。
    pub(super) fn bound_chunk(&self, text: &str) -> String {
        if text.len() <= TOOL_RESULT_MAX_INLINE_BYTES {
            return text.to_owned();
        }
        let notice = tool_result_omission_notice(text.len(), None);
        let budget = TOOL_RESULT_MAX_INLINE_BYTES.saturating_sub(notice.len());
        let mut bounded = retain_head_tail_bytes(text, budget);
        bounded.push_str(&notice);
        bounded
    }

    /// 完成态正文：裁剪 + 归档，并附完整输出路径。
    pub(super) fn bound_summary(&self, summary: &str, tool_name: &str) -> String {
        if summary.len() <= TOOL_RESULT_MAX_INLINE_BYTES {
            return summary.to_owned();
        }
        let archived = self.archive(summary, tool_name);
        let notice = tool_result_omission_notice(summary.len(), archived.as_deref());
        let budget = TOOL_RESULT_MAX_INLINE_BYTES.saturating_sub(notice.len());
        let mut bounded = retain_head_tail_bytes(summary, budget);
        bounded.push_str(&notice);
        bounded
    }

    /// metadata：整体超限时体积型字段换「计数 + 归档路径」，小字段照原样保留。
    pub(super) fn bound_metadata(&self, metadata: &Value, tool_name: &str) -> Value {
        let serialized = metadata.to_string();
        if serialized.len() <= TOOL_RESULT_MAX_INLINE_BYTES {
            return metadata.clone();
        }
        let archived = self.archive(&serialized, tool_name);
        let Some(object) = metadata.as_object() else {
            return omitted_entry(metadata, serialized.len());
        };
        let mut bounded = Map::new();
        for (key, value) in object {
            let value_bytes = value.to_string().len();
            if value_bytes <= METADATA_SCALAR_KEEP_BYTES {
                bounded.insert(key.clone(), value.clone());
            } else {
                bounded.insert(key.clone(), omitted_entry(value, value_bytes));
            }
        }
        bounded.insert("truncated".to_owned(), Value::Bool(true));
        if let Some(path) = archived {
            bounded.insert(METADATA_ARCHIVE_PATH_KEY.to_owned(), Value::String(path));
        }
        Value::Object(bounded)
    }

    /// 完整正文落归档，返回绝对路径；没有归档根或写入失败时返回 None。
    fn archive(&self, text: &str, tool_name: &str) -> Option<String> {
        let root = self.archive_root.as_ref()?;
        let source_trace = tool_output_source_trace_from_dispatch_name(tool_name);
        let preview = text.chars().take(ARCHIVE_PREVIEW_CHARS).collect::<String>();
        let archived = ContentArchiveRuntime::default()
            .archive_text_with_preview(
                root,
                CONTENT_ARCHIVE_RESULT_STREAM,
                text,
                &source_trace,
                &preview,
                true,
            )
            .ok()?;
        let output_ref = archived.output_ref?;
        let relative = output_ref.archive_ref.as_ref()?.relative_path.clone();
        Some(root.join(relative).display().to_string())
    }
}

/// 体积型字段的替身：体积与条数，正文见归档。
fn omitted_entry(value: &Value, bytes: usize) -> Value {
    let mut entry = Map::new();
    entry.insert("omitted".to_owned(), Value::Bool(true));
    entry.insert("bytes".to_owned(), Value::from(bytes));
    if let Some(items) = value.as_array() {
        entry.insert("count".to_owned(), Value::from(items.len()));
    }
    if let Some(text) = value.as_str() {
        entry.insert("chars".to_owned(), Value::from(text.chars().count()));
    }
    Value::Object(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_agent_runtime::TOOL_RESULT_OMISSION_SEPARATOR;

    #[test]
    fn oversized_summary_keeps_head_tail_and_states_omitted_bytes() {
        let budget = ToolResultBudget::default();
        let summary = format!("head-{}-tail", "x".repeat(TOOL_RESULT_MAX_INLINE_BYTES * 2));
        let bounded = budget.bound_summary(&summary, "builtin.search");
        assert!(
            bounded.len() <= TOOL_RESULT_MAX_INLINE_BYTES,
            "裁剪结果必须落在内联预算内，实际 {} 字节",
            bounded.len()
        );
        assert!(bounded.starts_with("head-"), "头部必须保留");
        assert!(bounded.contains("-tail"), "尾部必须保留");
        assert!(bounded.contains(TOOL_RESULT_OMISSION_SEPARATOR));
        assert!(
            bounded.contains(&format!("已省略 {} 字节", summary.len())),
            "省略量必须如实写出：{bounded}"
        );
    }

    #[test]
    fn within_budget_summary_is_untouched() {
        let budget = ToolResultBudget::default();
        let summary = "search entries=3";
        assert_eq!(budget.bound_summary(summary, "builtin.search"), summary);
    }

    #[test]
    fn oversized_chunk_is_trimmed_without_archive_path() {
        let budget = ToolResultBudget::default();
        let chunk = "y".repeat(TOOL_RESULT_MAX_INLINE_BYTES + 1);
        let bounded = budget.bound_chunk(&chunk);
        assert!(bounded.len() <= TOOL_RESULT_MAX_INLINE_BYTES);
        assert!(bounded.contains("已省略"));
        assert!(
            !bounded.contains("完整输出见"),
            "流式分片不落归档，不得给出不存在的路径"
        );
    }

    #[test]
    fn bulk_metadata_entries_become_counts_and_small_fields_survive() {
        let budget = ToolResultBudget::default();
        let metadata = serde_json::json!({
            "kind": "search",
            "query": "sso",
            "files": (0..2_000)
                .map(|index| serde_json::json!({"path": format!("src/file-{index:04}.ts")}))
                .collect::<Vec<_>>(),
        });
        let bounded = budget.bound_metadata(&metadata, "builtin.search");
        assert_eq!(bounded["kind"], "search", "小字段必须原样保留");
        assert_eq!(bounded["query"], "sso");
        assert_eq!(bounded["truncated"], true);
        assert_eq!(bounded["files"]["omitted"], true);
        assert_eq!(bounded["files"]["count"], 2_000);
        assert!(
            bounded["files"]["bytes"].as_u64().expect("体积") > 0,
            "替身必须给出被省略的体量"
        );
        assert!(
            bounded["files"].get("path").is_none(),
            "体积型正文不得留在事件里"
        );
        assert!(bounded.to_string().len() < metadata.to_string().len());
    }

    #[test]
    fn within_budget_metadata_is_untouched() {
        let budget = ToolResultBudget::default();
        let metadata = serde_json::json!({"kind": "search", "page": 1});
        assert_eq!(budget.bound_metadata(&metadata, "builtin.search"), metadata);
    }
}
