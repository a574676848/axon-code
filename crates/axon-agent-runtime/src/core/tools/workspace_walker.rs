//! search 与 glob 共用的工作区路径发现器。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::UNIX_EPOCH,
};

use axon_domain::{AxonError, AxonResult};
use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;

const WORKSPACE_WALK_GLOB_ERROR: &str = "workspace glob 无效";
const WORKSPACE_WALK_WORKER_ERROR: &str = "workspace 并行遍历 worker 异常退出";
const WORKSPACE_WALK_MIN_PARALLELISM: usize = 1;
const WORKSPACE_WALK_GIT_DIRECTORY: &str = ".git";
const WORKSPACE_WALK_NODE_MODULES_DIRECTORY: &str = "node_modules";

/// 工作区遍历取消令牌。
pub type WorkspaceWalkCancelToken = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

/// 默认路径为空时的遍历范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceWalkDefaultScope {
    /// 只返回 cwd 的直接子项。
    DirectChildren,
    /// 递归返回 cwd 下的文件。
    RecursiveFiles,
}

/// 条目类型过滤。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceWalkInclude {
    /// 只返回文件。
    Files,
    /// 只返回目录。
    Directories,
    /// 返回文件和目录。
    All,
}

/// 工作区路径发现输入。
#[derive(Clone)]
pub struct WorkspaceWalkInput {
    /// 工具 cwd。
    pub root: PathBuf,
    /// 字面文件、字面目录或 glob。
    pub paths: Vec<String>,
    /// 是否包含隐藏条目。
    pub hidden: bool,
    /// 是否尊重 Git ignore 规则。
    pub gitignore: bool,
    /// 条目类型过滤。
    pub include: WorkspaceWalkInclude,
    /// paths 为空或遇到字面目录时的遍历范围。
    pub default_scope: WorkspaceWalkDefaultScope,
    /// 协作式取消令牌。
    pub cancel_token: Option<WorkspaceWalkCancelToken>,
}

/// 工作区路径发现结果条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWalkEntry {
    /// 规范化绝对路径。
    pub path: PathBuf,
    /// 相对 cwd 的展示路径；显式 cwd 外路径使用绝对路径。
    pub display_path: String,
    /// 是否为目录。
    pub is_dir: bool,
    /// 文件字节长度；目录或元数据不可读时为 0。
    pub file_len: u64,
    /// 最后修改时间，Unix 毫秒。
    pub modified_at_ms: u128,
}

/// 工作区路径发现报告。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceWalkReport {
    /// 去重后的条目。
    pub entries: Vec<WorkspaceWalkEntry>,
    /// 不存在的输入路径。
    pub missing_paths: Vec<String>,
    /// 无法读取的输入或遍历路径。
    pub unreadable_paths: Vec<String>,
    /// 是否因取消或遍历错误只返回了部分结果。
    pub incomplete: bool,
    /// 跳过的符号链接数量。
    pub symlinks_skipped: usize,
    /// 跳过的扫描根目录逃逸数量。
    pub root_escapes_skipped: usize,
}

/// 统一工作区路径发现器。
#[derive(Debug, Default, Clone, Copy)]
pub struct WorkspaceWalker;

impl WorkspaceWalker {
    /// 解析路径并执行一次有界语义遍历；分页由调用方在统一排序后完成。
    pub fn walk(input: WorkspaceWalkInput) -> AxonResult<WorkspaceWalkReport> {
        let root = normalize_windows_path(&input.root)
            .canonicalize()
            .map(|path| normalize_windows_path(&path))
            .map_err(|_| AxonError::tool_invalid_arguments("workspace root 不可访问"))?;
        if !root.is_dir() {
            return Err(AxonError::tool_invalid_arguments(
                "workspace root 必须是目录",
            ));
        }
        let mut collector = WalkCollector::new(root, &input);
        if input.paths.is_empty() {
            collector.scan_default(input.default_scope);
        } else {
            let parallelism = thread::available_parallelism()
                .map_or(WORKSPACE_WALK_MIN_PARALLELISM, std::num::NonZeroUsize::get);
            for batch in input.paths.chunks(parallelism) {
                if collector.cancelled() {
                    collector.report.incomplete = true;
                    break;
                }
                let reports = thread::scope(|scope| -> AxonResult<Vec<WorkspaceWalkReport>> {
                    let handles = batch
                        .iter()
                        .map(|expression| {
                            let root = collector.root.clone();
                            let input = collector.input;
                            scope.spawn(move || {
                                let mut partial = WalkCollector::new(root, input);
                                partial.scan_expression(expression)?;
                                Ok(partial.finish())
                            })
                        })
                        .collect::<Vec<_>>();
                    handles
                        .into_iter()
                        .map(|handle| {
                            handle.join().map_err(|_| {
                                AxonError::tool_execution_failed(WORKSPACE_WALK_WORKER_ERROR)
                            })?
                        })
                        .collect()
                })?;
                for report in reports {
                    collector.merge(report);
                }
            }
        }
        Ok(collector.finish())
    }
}

struct WalkCollector<'a> {
    root: PathBuf,
    input: &'a WorkspaceWalkInput,
    entries: BTreeMap<PathBuf, WorkspaceWalkEntry>,
    report: WorkspaceWalkReport,
}

impl<'a> WalkCollector<'a> {
    fn new(root: PathBuf, input: &'a WorkspaceWalkInput) -> Self {
        Self {
            root,
            input,
            entries: BTreeMap::new(),
            report: WorkspaceWalkReport::default(),
        }
    }

    fn scan_default(&mut self, scope: WorkspaceWalkDefaultScope) {
        let max_depth = match scope {
            WorkspaceWalkDefaultScope::DirectChildren => Some(1),
            WorkspaceWalkDefaultScope::RecursiveFiles => None,
        };
        let root = self.root.clone();
        self.scan_tree(&root, max_depth, None, false);
    }

    fn scan_expression(&mut self, expression: &str) -> AxonResult<()> {
        let normalized = normalize_expression(expression);
        if mentions_path_component(&normalized, WORKSPACE_WALK_GIT_DIRECTORY) {
            return Ok(());
        }
        if !has_glob_meta(&normalized) {
            return self.scan_literal(expression, &normalized);
        }
        let matcher = GlobBuilder::new(&normalized)
            .literal_separator(true)
            .build()
            .map_err(|_| AxonError::tool_invalid_arguments(WORKSPACE_WALK_GLOB_ERROR))?
            .compile_matcher();
        let anchor = glob_anchor(&self.root, &normalized);
        if !anchor.exists() {
            self.report.missing_paths.push(expression.to_owned());
            return Ok(());
        }
        let anchor = match anchor.canonicalize() {
            Ok(anchor) => normalize_windows_path(&anchor),
            Err(_) => {
                self.report.unreadable_paths.push(expression.to_owned());
                self.report.incomplete = true;
                return Ok(());
            }
        };
        let max_depth = glob_walk_depth(&normalized);
        self.scan_tree(
            &anchor,
            max_depth,
            Some((&matcher, Path::new(&normalized))),
            mentions_path_component(&normalized, "node_modules"),
        );
        Ok(())
    }

    fn scan_literal(&mut self, original: &str, normalized: &str) -> AxonResult<()> {
        let requested = resolve_path(&self.root, normalized);
        if !requested.exists() {
            self.report.missing_paths.push(original.to_owned());
            return Ok(());
        }
        let canonical = match requested.canonicalize() {
            Ok(path) => normalize_windows_path(&path),
            Err(_) => {
                self.report.unreadable_paths.push(original.to_owned());
                self.report.incomplete = true;
                return Ok(());
            }
        };
        if canonical.is_file() {
            self.collect_path(&canonical, false);
        } else if canonical.is_dir() {
            let max_depth = match self.input.default_scope {
                WorkspaceWalkDefaultScope::DirectChildren => Some(1),
                WorkspaceWalkDefaultScope::RecursiveFiles => None,
            };
            self.scan_tree(
                &canonical,
                max_depth,
                None,
                mentions_path_component(normalized, "node_modules"),
            );
        } else {
            self.report.unreadable_paths.push(original.to_owned());
        }
        Ok(())
    }

    fn scan_tree(
        &mut self,
        scan_root: &Path,
        max_depth: Option<usize>,
        matcher: Option<(&GlobMatcher, &Path)>,
        allow_node_modules: bool,
    ) {
        let mut builder = WalkBuilder::new(scan_root);
        builder
            .follow_links(false)
            .hidden(!self.input.hidden)
            .git_ignore(self.input.gitignore)
            .git_global(self.input.gitignore)
            .git_exclude(self.input.gitignore)
            .parents(self.input.gitignore)
            .ignore(self.input.gitignore)
            .require_git(false)
            .filter_entry(move |entry| {
                let name = entry.file_name().to_string_lossy();
                name != WORKSPACE_WALK_GIT_DIRECTORY
                    && (allow_node_modules || name != WORKSPACE_WALK_NODE_MODULES_DIRECTORY)
            })
            .max_depth(max_depth);
        let scan_root = scan_root.to_path_buf();
        for entry in builder.build() {
            if self.cancelled() {
                self.report.incomplete = true;
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.report.incomplete = true;
                    self.report.unreadable_paths.push(error.to_string());
                    continue;
                }
            };
            if entry.depth() == 0 {
                continue;
            }
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                self.report.symlinks_skipped = self.report.symlinks_skipped.saturating_add(1);
                continue;
            }
            let canonical = match entry.path().canonicalize() {
                Ok(path) => {
                    let path = normalize_windows_path(&path);
                    if path.starts_with(&scan_root) {
                        path
                    } else {
                        self.report.root_escapes_skipped =
                            self.report.root_escapes_skipped.saturating_add(1);
                        continue;
                    }
                }
                Err(_) => {
                    self.report.incomplete = true;
                    self.report
                        .unreadable_paths
                        .push(display_path(&self.root, entry.path()));
                    continue;
                }
            };
            if let Some((matcher, expression)) = matcher
                && !matches_expression(matcher, expression, &self.root, &canonical)
            {
                continue;
            }
            self.collect_path(&canonical, file_type.is_dir());
        }
    }

    fn collect_path(&mut self, path: &Path, is_dir: bool) {
        let included = matches!(
            (self.input.include, is_dir),
            (WorkspaceWalkInclude::Files, false)
                | (WorkspaceWalkInclude::Directories, true)
                | (WorkspaceWalkInclude::All, _)
        );
        if !included {
            return;
        }
        let metadata = path.metadata().ok();
        let file_len = metadata.as_ref().map_or(0, std::fs::Metadata::len);
        let modified_at_ms = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis());
        self.entries
            .entry(path.to_path_buf())
            .or_insert_with(|| WorkspaceWalkEntry {
                path: path.to_path_buf(),
                display_path: display_path(&self.root, path),
                is_dir,
                file_len,
                modified_at_ms,
            });
    }

    fn cancelled(&self) -> bool {
        self.input
            .cancel_token
            .as_ref()
            .is_some_and(|token| token())
    }

    fn merge(&mut self, mut report: WorkspaceWalkReport) {
        for entry in report.entries {
            self.entries.entry(entry.path.clone()).or_insert(entry);
        }
        self.report.missing_paths.append(&mut report.missing_paths);
        self.report
            .unreadable_paths
            .append(&mut report.unreadable_paths);
        self.report.incomplete |= report.incomplete;
        self.report.symlinks_skipped = self
            .report
            .symlinks_skipped
            .saturating_add(report.symlinks_skipped);
        self.report.root_escapes_skipped = self
            .report
            .root_escapes_skipped
            .saturating_add(report.root_escapes_skipped);
    }

    fn finish(mut self) -> WorkspaceWalkReport {
        self.report.entries = self.entries.into_values().collect();
        deduplicate_strings(&mut self.report.missing_paths);
        deduplicate_strings(&mut self.report.unreadable_paths);
        self.report
    }
}

fn normalize_expression(expression: &str) -> String {
    normalize_windows_path(Path::new(expression.trim()))
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalize_windows_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    let normalized = value
        .strip_prefix(r"\\?\")
        .or_else(|| value.strip_prefix(r"\\.\"))
        .unwrap_or(&value);
    PathBuf::from(normalized)
}

fn has_glob_meta(expression: &str) -> bool {
    expression.contains(['*', '?', '[', '{'])
}

fn mentions_path_component(expression: &str, component: &str) -> bool {
    expression.split('/').any(|value| value == component)
}

fn glob_walk_depth(expression: &str) -> Option<usize> {
    if expression.contains("**") || expression.contains('{') {
        return None;
    }
    let segments = expression
        .split('/')
        .skip_while(|segment| !has_glob_meta(segment))
        .filter(|segment| !segment.is_empty())
        .count();
    Some(segments.max(1))
}

fn resolve_path(root: &Path, expression: &str) -> PathBuf {
    let path = PathBuf::from(expression);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn glob_anchor(root: &Path, expression: &str) -> PathBuf {
    let prefix = expression
        .split('/')
        .take_while(|component| !has_glob_meta(component))
        .collect::<Vec<_>>()
        .join("/");
    let anchor = PathBuf::from(prefix);
    let anchor = if anchor.as_os_str().is_empty() {
        root.to_path_buf()
    } else if anchor.is_absolute() {
        anchor
    } else {
        root.join(anchor)
    };
    if anchor.is_file() {
        anchor.parent().map_or(anchor.clone(), Path::to_path_buf)
    } else {
        anchor
    }
}

fn matches_expression(
    matcher: &GlobMatcher,
    expression: &Path,
    root: &Path,
    candidate: &Path,
) -> bool {
    if expression.is_absolute() {
        matcher.is_match(candidate.to_string_lossy().replace('\\', "/"))
    } else {
        let relative = candidate.strip_prefix(root).unwrap_or(candidate);
        matcher.is_match(relative.to_string_lossy().replace('\\', "/"))
    }
}

fn display_path(root: &Path, path: &Path) -> String {
    let display = path.strip_prefix(root).unwrap_or(path);
    display.to_string_lossy().replace('\\', "/")
}

fn deduplicate_strings(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn literal_directory_is_non_recursive_and_glob_controls_recursion() {
        let root = axon_test_support::test_temp_dir();
        fs::create_dir_all(root.path().join("src/nested")).unwrap();
        fs::write(root.path().join("src/lib.rs"), "").unwrap();
        fs::write(root.path().join("src/nested/mod.rs"), "").unwrap();

        let mut direct_input = input(root.path(), vec!["src"]);
        direct_input.default_scope = WorkspaceWalkDefaultScope::DirectChildren;
        let direct = WorkspaceWalker::walk(direct_input).unwrap();
        let recursive = WorkspaceWalker::walk(input(root.path(), vec!["src/**/*.rs"])).unwrap();

        assert!(
            direct
                .entries
                .iter()
                .any(|entry| entry.display_path == "src/lib.rs")
        );
        assert!(
            !direct
                .entries
                .iter()
                .any(|entry| entry.display_path.ends_with("mod.rs"))
        );
        assert_eq!(recursive.entries.len(), 2);
    }

    #[test]
    fn non_recursive_glob_does_not_walk_unmatchable_subtree() {
        let root = axon_test_support::test_temp_dir();
        fs::create_dir_all(root.path().join("src/nested/deeper")).unwrap();
        fs::write(root.path().join("src/lib.rs"), "").unwrap();
        fs::write(root.path().join("src/nested/deeper/mod.rs"), "").unwrap();
        let checks = Arc::new(AtomicUsize::new(0));
        let mut request = input(root.path(), vec!["src/*.rs"]);
        let checks_for_cancel = Arc::clone(&checks);
        request.cancel_token = Some(Arc::new(move || {
            checks_for_cancel.fetch_add(1, Ordering::Relaxed) >= 4
        }));

        let report = WorkspaceWalker::walk(request).unwrap();

        assert!(!report.incomplete);
        assert_eq!(
            report
                .entries
                .iter()
                .map(|entry| entry.display_path.as_str())
                .collect::<Vec<_>>(),
            ["src/lib.rs"]
        );
    }

    #[test]
    fn glob_wildcard_does_not_cross_path_separator() {
        let root = axon_test_support::test_temp_dir();
        fs::create_dir_all(root.path().join("src/nested")).unwrap();
        fs::write(root.path().join("src").join("main.rs"), "").unwrap();
        fs::write(root.path().join("src/nested").join("deep.rs"), "").unwrap();

        let report = WorkspaceWalker::walk(WorkspaceWalkInput {
            root: root.path().to_path_buf(),
            paths: vec!["src/*.rs".to_owned()],
            hidden: false,
            gitignore: false,
            include: WorkspaceWalkInclude::Files,
            default_scope: WorkspaceWalkDefaultScope::DirectChildren,
            cancel_token: None,
        })
        .unwrap();

        assert_eq!(
            report
                .entries
                .iter()
                .map(|entry| entry.display_path.as_str())
                .collect::<Vec<_>>(),
            ["src/main.rs"]
        );
    }

    #[test]
    fn walker_reports_missing_and_honors_gitignore() {
        let root = axon_test_support::test_temp_dir();
        fs::write(root.path().join(".gitignore"), "*.log\n").unwrap();
        fs::write(root.path().join("keep.rs"), "").unwrap();
        fs::write(root.path().join("skip.log"), "").unwrap();
        let mut request = input(root.path(), vec!["missing", "**/*"]);
        request.gitignore = true;

        let report = WorkspaceWalker::walk(request).unwrap();

        assert_eq!(report.missing_paths, vec!["missing"]);
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.display_path == "keep.rs")
        );
        assert!(
            !report
                .entries
                .iter()
                .any(|entry| entry.display_path == "skip.log")
        );
    }

    #[cfg(windows)]
    #[test]
    fn extended_windows_root_discovers_direct_children() {
        let root = axon_test_support::test_temp_dir();
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(root.path().join("README.md"), "").unwrap();

        let extended_root = PathBuf::from(format!(r"\\?\{}", root.path().display()));
        let report = WorkspaceWalker::walk(WorkspaceWalkInput {
            root: extended_root,
            paths: Vec::new(),
            hidden: false,
            gitignore: false,
            include: WorkspaceWalkInclude::All,
            default_scope: WorkspaceWalkDefaultScope::DirectChildren,
            cancel_token: None,
        })
        .unwrap();

        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.display_path == "README.md")
        );
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.display_path == "src")
        );
        assert!(!report.incomplete);
    }

    #[cfg(windows)]
    #[test]
    fn extended_windows_absolute_glob_matches_entries() {
        let root = axon_test_support::test_temp_dir();
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(root.path().join("src").join("main.rs"), "").unwrap();

        let extended_root = PathBuf::from(format!(r"\\?\{}", root.path().display()));
        let pattern = format!(r"\\?\{}\src\*.rs", root.path().display());
        let report = WorkspaceWalker::walk(WorkspaceWalkInput {
            root: extended_root,
            paths: vec![pattern],
            hidden: false,
            gitignore: false,
            include: WorkspaceWalkInclude::Files,
            default_scope: WorkspaceWalkDefaultScope::DirectChildren,
            cancel_token: None,
        })
        .unwrap();

        assert_eq!(
            report
                .entries
                .iter()
                .map(|entry| entry.display_path.as_str())
                .collect::<Vec<_>>(),
            ["src/main.rs"]
        );
    }

    fn input(root: &Path, paths: Vec<&str>) -> WorkspaceWalkInput {
        WorkspaceWalkInput {
            root: root.to_path_buf(),
            paths: paths.into_iter().map(str::to_owned).collect(),
            hidden: false,
            gitignore: false,
            include: WorkspaceWalkInclude::Files,
            default_scope: WorkspaceWalkDefaultScope::RecursiveFiles,
            cancel_token: None,
        }
    }
}

#[cfg(test)]
#[path = "workspace_walker_omp_semantics_tests.rs"]
mod omp_semantics_tests;
