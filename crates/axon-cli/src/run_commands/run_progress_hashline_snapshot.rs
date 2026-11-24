//! Hashline 流式只读 diff 快照投影。
//!
//! 与 PathDiff 不同，Hashline 的正文只携带**新内容**，旧行不在载荷里。要让用户提前看到
//! 真实 diff，预览必须按当前磁盘内容渲染（这与 omp `applyPartialTo(text, …)` 的做法一致）。
//! 读取严格限制在工作区内：相对路径、且路径组件只能是一般名称。

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use axon_agent_runtime::{HashlinePreviewFile, HashlinePreviewOperation, hashline_preview_files};
use axon_protocol::diff::FileChange;

use super::run_progress::RunToolDiffSnapshotView;

/// 把（可能仍在流式生成的）Hashline 载荷投影为只读 diff 快照。
///
/// 标签陈旧、目标文件缺失、操作尚未写完时对应文件被跳过；全部跳过时返回 `None`，
/// 调用方据此不发出任何 diff 事件（预览绝不把半截输入当成最终结果）。
pub(super) fn hashline_diff_snapshot(
    workspace_root: &Path,
    provider_call_id: &str,
    tool_part_id: &str,
    input: &str,
) -> Option<RunToolDiffSnapshotView> {
    let lookup = |path: &str| read_workspace_text(workspace_root, path);
    let files = hashline_preview_files(input, &lookup);
    if files.is_empty() {
        return None;
    }
    let mut changes = BTreeMap::new();
    for HashlinePreviewFile {
        path,
        operation,
        unified_diff,
    } in files
    {
        let change = match operation {
            HashlinePreviewOperation::Delete => FileChange::Delete {
                content: String::new(),
            },
            HashlinePreviewOperation::Update => FileChange::Update {
                unified_diff,
                move_path: None,
            },
            HashlinePreviewOperation::Move { destination } => FileChange::Update {
                unified_diff,
                move_path: Some(PathBuf::from(destination)),
            },
        };
        changes.insert(PathBuf::from(path), change);
    }
    Some(RunToolDiffSnapshotView {
        tool_part_id: tool_part_id.to_owned(),
        provider_call_id: provider_call_id.to_owned(),
        changes,
    })
}

/// 工作区内文本读取；绝对路径、父目录跳转、前缀组件一律拒绝。
fn read_workspace_text(workspace_root: &Path, path: &str) -> Option<String> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    std::fs::read_to_string(workspace_root.join(relative)).ok()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static PREVIEW_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn test_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "axon-cli-hashline-preview-{tag}-{}-{}",
            std::process::id(),
            PREVIEW_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn update_diff<'a>(snapshot: &'a RunToolDiffSnapshotView, path: &str) -> &'a str {
        match snapshot.changes.get(&PathBuf::from(path)) {
            Some(FileChange::Update { unified_diff, .. }) => unified_diff,
            other => panic!("期望 {path} 为 update，实际 {other:?}"),
        }
    }

    #[test]
    fn renders_a_real_diff_from_the_current_file_content() {
        let root = test_root("real-diff");
        let content = "fn main() {\n    let a = 1;\n}\n";
        fs::write(root.join("src.rs"), content).unwrap();
        let tag = axon_agent_runtime::hashline_snapshot_tag(content);
        let input = format!("[src.rs#{tag}]\nSWAP 2.=2:\n+    let a = 2;\n");

        let snapshot =
            hashline_diff_snapshot(&root, "call-1", "part-1", &input).expect("应生成预览");

        let diff = update_diff(&snapshot, "src.rs");
        assert!(diff.contains("-    let a = 1;"), "diff={diff}");
        assert!(diff.contains("+    let a = 2;"), "diff={diff}");
        // 未被触及的上下文行不应出现在 diff 里。
        assert!(!diff.contains("fn main"), "diff={diff}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tolerates_a_half_written_trailing_operation() {
        let root = test_root("half-written");
        let content = "one\ntwo\nthree\n";
        fs::write(root.join("doc.txt"), content).unwrap();
        let tag = axon_agent_runtime::hashline_snapshot_tag(content);
        let input = format!("[doc.txt#{tag}]\nDEL 3.=3\nSWAP 1.=1:");

        let snapshot =
            hashline_diff_snapshot(&root, "call-2", "part-2", &input).expect("已完成部分应可预览");

        let diff = update_diff(&snapshot, "doc.txt");
        assert!(diff.contains("-three"), "diff={diff}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_stale_tags_missing_files_and_unsafe_paths() {
        let root = test_root("skips");
        let content = "alpha\nbeta\n";
        fs::write(root.join("doc.txt"), content).unwrap();
        fs::write(root.join("other.txt"), "outside\n").unwrap();
        let stale = "[doc.txt#FFFFFF]\nDEL 1.=1\n".to_owned();
        let missing = format!(
            "[nope.txt#{}]\nDEL 1.=1\n",
            axon_agent_runtime::hashline_snapshot_tag("nope\n")
        );
        let escaping = format!(
            "[../escaped.txt#{}]\nDEL 1.=1\n",
            axon_agent_runtime::hashline_snapshot_tag("outside\n")
        );

        assert!(hashline_diff_snapshot(&root, "c", "p", &stale).is_none());
        assert!(hashline_diff_snapshot(&root, "c", "p", &missing).is_none());
        assert!(hashline_diff_snapshot(&root, "c", "p", &escaping).is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
