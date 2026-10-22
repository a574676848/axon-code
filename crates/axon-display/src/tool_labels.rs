//! Desktop 与 TUI 共用的内置工具中文名称。

use std::collections::BTreeMap;
use std::sync::OnceLock;

const BUILTIN_NAMESPACE_PREFIX: &str = "builtin.";
const BUILTIN_TOOL_LABELS_JSON: &str = include_str!("builtin_tool_labels.json");
static BUILTIN_TOOL_LABELS: OnceLock<BTreeMap<String, String>> = OnceLock::new();

/// 根据完整内置名称或无命名空间名称查询标签，其他命名空间不参与匹配。
#[must_use]
pub fn builtin_tool_display_label(tool_name: &str) -> Option<&'static str> {
    let name = tool_name
        .trim()
        .strip_prefix(BUILTIN_NAMESPACE_PREFIX)
        .unwrap_or(tool_name.trim());
    BUILTIN_TOOL_LABELS
        .get_or_init(|| {
            serde_json::from_str(BUILTIN_TOOL_LABELS_JSON)
                .expect("内置工具中文名称表必须是有效 JSON")
        })
        .get(name)
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_valid_and_only_match_builtin_names() {
        let labels: BTreeMap<String, String> =
            serde_json::from_str(BUILTIN_TOOL_LABELS_JSON).expect("名称表解析失败");
        assert!(!labels.is_empty());
        for (name, label) in labels {
            assert!(!label.trim().is_empty(), "{name}");
            assert_eq!(builtin_tool_display_label(&name), Some(label.as_str()));
            assert_eq!(
                builtin_tool_display_label(&format!("builtin.{name}")),
                Some(label.as_str())
            );
            assert_eq!(builtin_tool_display_label(&format!("plugin.{name}")), None);
        }
        assert_eq!(builtin_tool_display_label("notes_unknown"), None);
        assert_eq!(builtin_tool_display_label("mcp__server__read_file"), None);
    }
}
