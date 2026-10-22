//! 工具文件变更 metadata 到 wire FileChange 的目标态投影。

use std::{collections::BTreeMap, path::PathBuf};

use axon_protocol::diff::FileChange;
use serde_json::Value;

const DIFF_PART_PREFIX: &str = "diff";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProjectedToolFileChange {
    pub(super) part_id: String,
    pub(super) path: String,
    pub(super) change: FileChange,
}

pub(super) fn project_tool_file_changes(
    tool_part_id: &str,
    metadata: Option<&Value>,
) -> Vec<ProjectedToolFileChange> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    let mut changes = file_changes_from_metadata(metadata);
    if changes.is_empty() {
        changes = single_file_write_from_metadata(metadata)
            .into_iter()
            .collect();
    }
    let changes = retain_latest_file_changes(changes, None);
    changes
        .into_iter()
        .map(|(path, change)| {
            let path_text = path.display().to_string();
            ProjectedToolFileChange {
                part_id: diff_part_id(tool_part_id, &path_text),
                path: path_text,
                change,
            }
        })
        .collect()
}

fn file_changes_from_metadata(metadata: &Value) -> Vec<(PathBuf, FileChange)> {
    let mut changes = Vec::new();
    for file in ["files", "fileChanges", "changes"]
        .into_iter()
        .filter_map(|key| metadata.get(key).and_then(Value::as_array))
        .flatten()
    {
        if let Some(change) = file_change_from_metadata(file) {
            changes.push(change);
        }
    }
    if !changes.is_empty() {
        return changes;
    }
    grouped_file_changes(metadata)
}

fn file_change_from_metadata(file: &Value) -> Option<(PathBuf, FileChange)> {
    let kind = file_change_kind_value(file).map_or_else(|| "update".to_owned(), change_kind);
    match kind.as_str() {
        "add" => file_change_path(file).map(|path| {
            (
                PathBuf::from(path),
                FileChange::Add {
                    content: file_change_diff_text(file).unwrap_or_default(),
                },
            )
        }),
        "delete" => file_change_path(file).map(|path| {
            (
                PathBuf::from(path),
                FileChange::Delete {
                    content: file_change_diff_text(file).unwrap_or_default(),
                },
            )
        }),
        "move" => rename_file_change_from_metadata(file),
        _ => file_change_path(file).map(|path| {
            (
                PathBuf::from(path),
                FileChange::Update {
                    unified_diff: file_change_diff_text(file).unwrap_or_default(),
                    move_path: None,
                },
            )
        }),
    }
}

fn rename_file_change_from_metadata(file: &Value) -> Option<(PathBuf, FileChange)> {
    let target = json_string(file, "newPath")
        .or_else(|| json_string(file, "to"))
        .or_else(|| json_string(file, "target"))
        .or_else(|| json_string(file, "path"))
        .or_else(|| json_string(file, "file"))
        .or_else(|| json_string(file, "filePath"))?;
    let source = json_string(file, "oldPath")
        .or_else(|| json_string(file, "from"))
        .or_else(|| json_string(file, "source"))
        .or_else(|| json_string(file, "sourcePath"))
        .or_else(|| json_string(file, "previousPath"))
        .or_else(|| file_change_kind_value(file).and_then(move_path))
        .filter(|path| path != &target)?;
    Some((
        PathBuf::from(source),
        FileChange::Update {
            unified_diff: file_change_diff_text(file).unwrap_or_default(),
            move_path: Some(PathBuf::from(target)),
        },
    ))
}

fn grouped_file_changes(metadata: &Value) -> Vec<(PathBuf, FileChange)> {
    let mut changes = Vec::new();
    for path in file_list(metadata, "addedFiles").chain(file_list(metadata, "createdFiles")) {
        changes.push((
            PathBuf::from(path),
            FileChange::Add {
                content: String::new(),
            },
        ));
    }
    for path in file_list(metadata, "deletedFiles").chain(file_list(metadata, "removedFiles")) {
        changes.push((
            PathBuf::from(path),
            FileChange::Delete {
                content: String::new(),
            },
        ));
    }
    for path in file_list(metadata, "modifiedFiles").chain(file_list(metadata, "updatedFiles")) {
        changes.push((
            PathBuf::from(path),
            FileChange::Update {
                unified_diff: String::new(),
                move_path: None,
            },
        ));
    }
    for path in file_list(metadata, "renamedFiles") {
        if let Some((source, target)) = split_rename_path(&path) {
            changes.push((
                PathBuf::from(source),
                FileChange::Update {
                    unified_diff: String::new(),
                    move_path: Some(PathBuf::from(target)),
                },
            ));
        }
    }
    changes
}

fn single_file_write_from_metadata(metadata: &Value) -> Option<(PathBuf, FileChange)> {
    let path = json_string(metadata, "path")?;
    let writes = json_usize(metadata, "textWritesApplied");
    let renames = json_usize(metadata, "renamesApplied");
    if writes == 0 && renames == 0 && !metadata_has_file_write_marker(metadata) {
        return None;
    }
    Some((
        PathBuf::from(path),
        FileChange::Update {
            unified_diff: String::new(),
            move_path: None,
        },
    ))
}

fn retain_latest_file_changes(
    changes: impl IntoIterator<Item = (PathBuf, FileChange)>,
    max_items: Option<usize>,
) -> Vec<(PathBuf, FileChange)> {
    let mut latest = BTreeMap::new();
    for (path, change) in changes {
        if let Some(limit) = max_items
            && latest.len() >= limit
            && !latest.contains_key(&path)
        {
            continue;
        }
        latest.insert(path, change);
    }
    latest.into_iter().collect()
}

pub(super) fn diff_part_id(tool_part_id: &str, path: &str) -> String {
    let normalized_path = path.trim().replace('\\', "/");
    if normalized_path.is_empty() {
        format!("{DIFF_PART_PREFIX}:{tool_part_id}")
    } else {
        format!("{DIFF_PART_PREFIX}:{tool_part_id}:{normalized_path}")
    }
}

fn file_change_kind_value(file: &Value) -> Option<&Value> {
    ["status", "kind", "changeType", "operation", "type"]
        .into_iter()
        .find_map(|key| file.get(key))
}

fn file_change_path(file: &Value) -> Option<String> {
    json_string(file, "path")
        .or_else(|| json_string(file, "file"))
        .or_else(|| json_string(file, "filePath"))
}

fn file_change_diff_text(file: &Value) -> Option<String> {
    ["diff", "patch", "unifiedDiff", "unified_diff", "content"]
        .into_iter()
        .find_map(|key| raw_json_string(file, key))
}

fn raw_json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn change_kind(value: &Value) -> String {
    if let Some(kind) = value.as_str() {
        return normalized_kind(kind);
    }
    value
        .get("type")
        .or_else(|| value.get("operation"))
        .and_then(Value::as_str)
        .map_or_else(|| "update".to_owned(), normalized_kind)
}

fn normalized_kind(value: &str) -> String {
    match value
        .replace(['_', '-', '/'], "")
        .to_ascii_lowercase()
        .as_str()
    {
        "added" | "add" | "created" | "create" | "new" => "add".to_owned(),
        "deleted" | "delete" | "removed" | "remove" => "delete".to_owned(),
        "renamed" | "rename" | "moved" | "move" => "move".to_owned(),
        _ => "update".to_owned(),
    }
}

fn file_list<'a>(metadata: &'a Value, key: &str) -> impl Iterator<Item = String> + 'a {
    metadata
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(file_list_value)
}

fn file_list_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| json_string(value, "path"))
        .or_else(|| json_string(value, "file"))
        .or_else(|| json_string(value, "filePath"))
}

fn split_rename_path(path: &str) -> Option<(String, String)> {
    let (source, target) = path
        .split_once("->")
        .or_else(|| path.split_once("=>"))
        .map(|(source, target)| (source.trim(), target.trim()))?;
    (!source.is_empty() && !target.is_empty()).then(|| (source.to_owned(), target.to_owned()))
}

fn metadata_has_file_write_marker(metadata: &Value) -> bool {
    matches!(json_string(metadata, "kind").as_deref(), Some("edit"))
}

fn json_usize(value: &Value, key: &str) -> usize {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_default()
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn move_path(kind: &Value) -> Option<String> {
    kind.get("movePath")
        .or_else(|| kind.get("move_path"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_keeps_latest_change_for_duplicate_path() {
        let metadata = serde_json::json!({
            "files": [
                {
                    "path": "src/error.rs",
                    "kind": "add",
                    "diff": "old\n"
                },
                {
                    "path": "src/error.rs",
                    "kind": "add",
                    "diff": "new\n"
                }
            ]
        });

        let changes = project_tool_file_changes("tool-part:test", Some(&metadata));

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "src/error.rs");
        assert!(matches!(
            &changes[0].change,
            FileChange::Add { content } if content == "new\n"
        ));
    }
}
