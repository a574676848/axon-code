//! `ignore` 驱动的统一文件遍历入口。

use std::path::{Path, PathBuf};

use axon_domain::AxonResult;
use ignore::{DirEntry, WalkBuilder};

use crate::entry::{EntryKind, WalkEntry};
use crate::error::{root_not_directory, root_unreadable};
use crate::policy::DirPolicy;

/// 遍历深度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WalkDepth {
    /// 只访问根目录下一层条目。
    One,
    /// 不限制深度。
    #[default]
    Unlimited,
    /// 最多访问指定深度。
    Limited(usize),
}

/// visitor 对遍历流程的控制信号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkAction {
    /// 继续遍历。
    Continue,
    /// 停止遍历，并把结果标记为截断。
    Stop,
}

/// 遍历配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkOpts {
    /// 深度策略。
    pub depth: WalkDepth,
    /// 目录跳过策略。
    pub dir_policy: DirPolicy,
    /// 是否跟随符号链接。
    pub follow_symlinks: bool,
    /// 输出条目上限。
    pub max_entries: Option<usize>,
    /// 只输出文件。
    pub require_file: bool,
    /// 只输出目录。
    pub require_dir: bool,
    /// 可选扩展名过滤，不包含点号。
    pub extensions: Option<Vec<String>>,
    /// 可选文件大小上限。
    pub max_file_bytes: Option<u64>,
}

impl Default for WalkOpts {
    fn default() -> Self {
        Self {
            depth: WalkDepth::Unlimited,
            dir_policy: DirPolicy::Default,
            follow_symlinks: false,
            max_entries: None,
            require_file: false,
            require_dir: false,
            extensions: None,
            max_file_bytes: None,
        }
    }
}

/// 遍历汇总。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WalkSummary {
    /// 已看见的条目数。
    pub entries_seen: usize,
    /// 已输出给 visitor 的条目数。
    pub entries_emitted: usize,
    /// 是否因为错误、上限或 visitor 停止而截断。
    pub truncated: bool,
}

/// fd 风格遍历服务。
#[derive(Debug, Default, Clone, Copy)]
pub struct Walk;

impl Walk {
    /// 执行遍历，并对每个通过过滤的条目调用 visitor。
    pub fn run<F>(
        root: impl AsRef<Path>,
        opts: &WalkOpts,
        mut visitor: F,
    ) -> AxonResult<WalkSummary>
    where
        F: FnMut(&WalkEntry) -> WalkAction,
    {
        let root = root.as_ref();
        validate_root(root)?;

        let mut builder = WalkBuilder::new(root);
        builder
            .standard_filters(true)
            .follow_links(opts.follow_symlinks)
            .sort_by_file_path(std::cmp::Ord::cmp);
        apply_depth(&mut builder, opts.depth);
        builder.filter_entry({
            let policy = opts.dir_policy.clone();
            move |entry| filter_entry(&policy, entry)
        });

        let mut summary = WalkSummary::default();
        for entry in builder.build() {
            let Ok(entry) = entry else {
                summary.truncated = true;
                continue;
            };
            if is_root_entry(root, &entry) {
                continue;
            }
            summary.entries_seen = summary.entries_seen.saturating_add(1);
            let Some(walk_entry) = project_entry(root, &entry) else {
                continue;
            };
            if !allows_entry(opts, &walk_entry) {
                continue;
            }
            if opts
                .max_entries
                .is_some_and(|limit| summary.entries_emitted >= limit)
            {
                summary.truncated = true;
                break;
            }
            summary.entries_emitted = summary.entries_emitted.saturating_add(1);
            if visitor(&walk_entry) == WalkAction::Stop {
                summary.truncated = true;
                break;
            }
        }
        Ok(summary)
    }
}

fn validate_root(root: &Path) -> AxonResult<()> {
    let metadata = root.metadata().map_err(|_| root_unreadable())?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(root_not_directory())
    }
}

fn apply_depth(builder: &mut WalkBuilder, depth: WalkDepth) {
    match depth {
        WalkDepth::One => {
            builder.max_depth(Some(1));
        }
        WalkDepth::Unlimited => {}
        WalkDepth::Limited(max_depth) => {
            builder.max_depth(Some(max_depth));
        }
    }
}

fn filter_entry(policy: &DirPolicy, entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let Some(file_type) = entry.file_type() else {
        return true;
    };
    if !file_type.is_dir() {
        return true;
    }
    entry
        .file_name()
        .to_str()
        .is_none_or(|name| !policy.skips(name))
}

fn is_root_entry(root: &Path, entry: &DirEntry) -> bool {
    entry.depth() == 0 || entry.path() == root
}

fn project_entry(root: &Path, entry: &DirEntry) -> Option<WalkEntry> {
    let file_type = entry.file_type()?;
    let kind = if file_type.is_file() {
        EntryKind::File
    } else if file_type.is_dir() {
        EntryKind::Dir
    } else if file_type.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::Other
    };
    let path = entry.path().to_path_buf();
    let relative = path
        .strip_prefix(root)
        .map_or_else(|_| PathBuf::from(&path), Path::to_path_buf);
    let file_size = if kind == EntryKind::File {
        entry.metadata().ok().map(|metadata| metadata.len())
    } else {
        None
    };
    Some(WalkEntry::new(
        path,
        relative,
        kind,
        entry.depth(),
        file_size,
    ))
}

fn allows_entry(opts: &WalkOpts, entry: &WalkEntry) -> bool {
    if opts.require_file && entry.kind != EntryKind::File {
        return false;
    }
    if opts.require_dir && entry.kind != EntryKind::Dir {
        return false;
    }
    if entry.kind == EntryKind::File
        && opts
            .max_file_bytes
            .zip(entry.file_size)
            .is_some_and(|(limit, size)| size > limit)
    {
        return false;
    }
    if let Some(extensions) = &opts.extensions
        && entry.kind == EntryKind::File
    {
        return entry
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|actual| {
                extensions
                    .iter()
                    .any(|expected| actual.eq_ignore_ascii_case(expected))
            });
    }
    true
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Walk, WalkAction, WalkDepth, WalkOpts};
    use crate::{DirPolicy, EntryKind};

    #[test]
    fn walk_skips_default_generated_directories() {
        let root = temp_root("default-skip");
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules").join("hidden.rs"), "").unwrap();
        fs::write(root.join("visible.rs"), "").unwrap();

        let mut paths = Vec::new();
        let summary = Walk::run(&root, &WalkOpts::default(), |entry| {
            paths.push(entry.relative.clone());
            WalkAction::Continue
        })
        .unwrap();

        assert!(!summary.truncated);
        assert!(paths.iter().any(|path| path.ends_with("visible.rs")));
        assert!(!paths.iter().any(|path| path.ends_with("hidden.rs")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn walk_one_depth_emits_only_direct_children() {
        let root = temp_root("one-depth");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("lib.rs"), "").unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();

        let opts = WalkOpts {
            depth: WalkDepth::One,
            ..WalkOpts::default()
        };
        let mut files = Vec::new();
        Walk::run(&root, &opts, |entry| {
            if entry.kind == EntryKind::File {
                files.push(entry.relative.clone());
            }
            WalkAction::Continue
        })
        .unwrap();

        assert!(files.iter().any(|path| path.ends_with("Cargo.toml")));
        assert!(!files.iter().any(|path| path.ends_with("lib.rs")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn walk_marks_truncated_when_max_entries_is_reached() {
        let root = temp_root("max-entries");
        fs::write(root.join("a.rs"), "").unwrap();
        fs::write(root.join("b.rs"), "").unwrap();

        let opts = WalkOpts {
            max_entries: Some(1),
            require_file: true,
            ..WalkOpts::default()
        };
        let mut count = 0;
        let summary = Walk::run(&root, &opts, |_| {
            count += 1;
            WalkAction::Continue
        })
        .unwrap();

        assert_eq!(count, 1);
        assert!(summary.truncated);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn walk_custom_policy_can_disable_skips() {
        let root = temp_root("no-skip");
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target").join("debug.log"), "").unwrap();
        let opts = WalkOpts {
            dir_policy: DirPolicy::None,
            require_file: true,
            ..WalkOpts::default()
        };
        let mut files = Vec::new();
        Walk::run(&root, &opts, |entry| {
            files.push(entry.relative.clone());
            WalkAction::Continue
        })
        .unwrap();

        assert!(files.iter().any(|path| path.ends_with("debug.log")));
        let _ = fs::remove_dir_all(root);
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-fs-walk-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
