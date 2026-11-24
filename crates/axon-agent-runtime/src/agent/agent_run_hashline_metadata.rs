//! Hashline 快照回执元数据。

use crate::hashline_snapshot_tag;

/// 构造文件级 Hashline 快照元数据。
#[must_use]
pub fn hashline_file_metadata(path: &str, content: &str) -> serde_json::Value {
    let tag = hashline_snapshot_tag(content);
    serde_json::json!({
        "path": path,
        "tag": tag,
        "header": hashline_header(path, &tag),
    })
}

/// 将最终文件内容对应的 Hashline 快照注入文件变更元数据。
#[must_use]
pub fn file_change_with_hashline_snapshot(
    mut file_change: serde_json::Value,
    path: &str,
    content: &str,
) -> serde_json::Value {
    if let Some(object) = file_change.as_object_mut() {
        object.insert("hashline".to_owned(), hashline_file_metadata(path, content));
    }
    file_change
}

fn hashline_header(path: &str, tag: &str) -> String {
    format!("[{path}#{tag}]")
}
