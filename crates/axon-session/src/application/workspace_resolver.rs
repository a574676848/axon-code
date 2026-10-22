//! Workspace 身份与别名解析应用服务。

use std::{
    io,
    path::{Path, PathBuf},
    thread,
};

use axon_process::non_interactive_command;

use crate::domain::WorkspaceRecord;

const WORKSPACE_ID_PREFIX: &str = "workspace-";
const MEMORY_NAMESPACE_PREFIX: &str = "workspace:";
const FALLBACK_WORKSPACE_NAME: &str = "workspace";
const GIT_DIRECTORY_NAME: &str = ".git";
const GIT_EXECUTABLE: &str = "git";
const REMOTE_ORIGIN_ARGUMENTS: &[&str] = &["config", "--get", "remote.origin.url"];
const GIT_STATUS_ARGUMENTS: &[&str] = &["status", "--porcelain", "--branch"];
const GIT_BRANCH_LINE_PREFIX: &str = "## ";
const GIT_UNBORN_BRANCH_PREFIX: &str = "No commits yet on ";
const GIT_DETACHED_BRANCH: &str = "HEAD";
const GIT_UPSTREAM_SEPARATOR: &str = "...";
const GIT_VALUE_ABSENT_EXIT_CODE: i32 = 1;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Workspace 解析失败。
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceResolverError {
    /// 当前工作目录不可访问。
    #[error("工作目录不可访问：{path}：{source}")]
    InaccessibleCwd {
        /// 原始工作目录。
        path: String,
        /// IO 失败原因。
        #[source]
        source: io::Error,
    },
}

/// Workspace 注册信息解析器。
#[derive(Debug, Default, Clone, Copy)]
pub struct WorkspaceResolver;

/// Git 字段采集状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSnapshotFieldState {
    /// 字段已成功采集。
    Collected,
    /// Git 仓库未提供该字段。
    Absent,
    /// Git 命令或输出解析失败。
    Failed,
    /// 当前目录不是 Git Workspace。
    NotApplicable,
}

/// 单 turn 内复用的 Git 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitSnapshot {
    /// origin remote。
    pub remote: Option<String>,
    /// 当前分支；detached HEAD 时为空。
    pub branch: Option<String>,
    /// Workspace 是否存在未提交改动；采集失败时为空。
    pub dirty: Option<bool>,
    /// remote 字段采集状态。
    pub remote_state: WorkspaceSnapshotFieldState,
    /// branch 字段采集状态。
    pub branch_state: WorkspaceSnapshotFieldState,
    /// dirty 字段采集状态。
    pub dirty_state: WorkspaceSnapshotFieldState,
    /// 本次 remote Git 子进程数。
    pub remote_probe_count: usize,
    /// 本次 status Git 子进程数。
    pub status_probe_count: usize,
}

/// 单 turn 内复用的 Workspace/Git 事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    /// 调用方传入 cwd 的规范化结果，用于保持 Memory namespace 语义。
    pub requested_cwd: PathBuf,
    /// Git 根目录或独立 cwd。
    pub workspace_root: PathBuf,
    /// Workspace 注册记录。
    pub workspace: WorkspaceRecord,
    /// Git 快照。
    pub git: WorkspaceGitSnapshot,
}

impl WorkspaceResolver {
    /// 基于 cwd 解析当前 Workspace 注册信息。
    pub fn resolve(
        &self,
        cwd: &Path,
        now_ms: u64,
    ) -> Result<WorkspaceRecord, WorkspaceResolverError> {
        self.resolve_snapshot(cwd, now_ms)
            .map(|snapshot| snapshot.workspace)
    }

    /// 一次解析本 turn 的 Workspace 注册信息与 Git 元数据。
    pub fn resolve_snapshot(
        &self,
        cwd: &Path,
        now_ms: u64,
    ) -> Result<WorkspaceSnapshot, WorkspaceResolverError> {
        let requested_cwd = canonicalize_cwd(cwd)?;
        let workspace_root = find_workspace_root(&requested_cwd);
        let git = WorkspaceGitSnapshot::collect(&workspace_root);
        let workspace = self.resolve_identity(
            WorkspaceIdentity {
                workspace_root: workspace_root.clone(),
                git_remote: git.remote.clone(),
                default_branch: git.branch.clone(),
            },
            now_ms,
        );
        Ok(WorkspaceSnapshot {
            requested_cwd,
            workspace_root,
            workspace,
            git,
        })
    }

    /// 将已解析的 Git 身份转换为 Workspace 注册信息。
    #[must_use]
    pub fn resolve_identity(&self, identity: WorkspaceIdentity, now_ms: u64) -> WorkspaceRecord {
        let workspace_root = normalize_path(&identity.workspace_root);
        let workspace_id = stable_workspace_id(
            &workspace_root,
            identity.git_remote.as_deref(),
            identity.default_branch.as_deref(),
        );
        WorkspaceRecord {
            memory_namespace: format!("{MEMORY_NAMESPACE_PREFIX}{workspace_id}"),
            workspace_id,
            display_name: identity
                .workspace_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(FALLBACK_WORKSPACE_NAME)
                .to_owned(),
            workspace_root,
            git_remote: identity.git_remote,
            default_branch: identity.default_branch,
            aliases: Vec::new(),
            pinned: false,
            last_seen_at_ms: now_ms,
        }
    }

    /// 以 Git identity 重绑定已移动的 Workspace，并保留旧目录别名。
    ///
    /// 同一 remote/branch 的活跃 clone 保持独立身份；只有原根目录已失效时，
    /// 才将其重绑定到本次解析出的 cwd。
    #[must_use]
    pub fn reconcile_alias(
        &self,
        candidate: WorkspaceRecord,
        known_workspaces: &[WorkspaceRecord],
    ) -> WorkspaceRecord {
        let Some(existing) = known_workspaces.iter().find(|existing| {
            existing.git_remote.is_some()
                && existing.git_remote == candidate.git_remote
                && existing.default_branch.is_some()
                && existing.default_branch == candidate.default_branch
                && existing.workspace_root != candidate.workspace_root
                && !Path::new(&existing.workspace_root).exists()
        }) else {
            return candidate;
        };
        let mut rebound = existing.clone();
        if !rebound.aliases.contains(&existing.workspace_root) {
            rebound.aliases.push(existing.workspace_root.clone());
        }
        rebound.workspace_root = candidate.workspace_root;
        rebound.display_name = candidate.display_name;
        rebound.last_seen_at_ms = candidate.last_seen_at_ms;
        rebound
    }
}

/// 用于构造 Workspace 注册信息的环境身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceIdentity {
    /// Git 根目录或独立 cwd。
    pub workspace_root: PathBuf,
    /// origin remote。
    pub git_remote: Option<String>,
    /// 当前默认分支。
    pub default_branch: Option<String>,
}

impl WorkspaceGitSnapshot {
    fn collect(workspace_root: &Path) -> Self {
        if !workspace_root.join(GIT_DIRECTORY_NAME).exists() {
            return Self {
                remote: None,
                branch: None,
                dirty: None,
                remote_state: WorkspaceSnapshotFieldState::NotApplicable,
                branch_state: WorkspaceSnapshotFieldState::NotApplicable,
                dirty_state: WorkspaceSnapshotFieldState::NotApplicable,
                remote_probe_count: 0,
                status_probe_count: 0,
            };
        }

        let ((remote, mut remote_state), (branch, dirty, branch_state, dirty_state)) =
            thread::scope(|scope| {
                let remote_probe = scope.spawn(|| git_remote(workspace_root));
                let status = git_status(workspace_root);
                (
                    remote_probe.join().expect("Git remote 探测线程不应 panic"),
                    status,
                )
            });
        if remote_state == WorkspaceSnapshotFieldState::Absent
            && dirty_state == WorkspaceSnapshotFieldState::Failed
        {
            remote_state = WorkspaceSnapshotFieldState::Failed;
        }
        Self {
            remote,
            branch,
            dirty,
            remote_state,
            branch_state,
            dirty_state,
            remote_probe_count: 1,
            status_probe_count: 1,
        }
    }
}

fn canonicalize_cwd(cwd: &Path) -> Result<PathBuf, WorkspaceResolverError> {
    cwd.canonicalize()
        .map_err(|source| WorkspaceResolverError::InaccessibleCwd {
            path: cwd.display().to_string(),
            source,
        })
}

fn find_workspace_root(candidate: &Path) -> PathBuf {
    let mut current = candidate;
    loop {
        if current.join(GIT_DIRECTORY_NAME).exists() {
            return current.to_path_buf();
        }
        let Some(parent) = current.parent() else {
            return candidate.to_path_buf();
        };
        current = parent;
    }
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn stable_workspace_id(root: &str, remote: Option<&str>, branch: Option<&str>) -> String {
    let identity = format!(
        "{}{}{}",
        root,
        remote.unwrap_or_default(),
        branch.unwrap_or_default()
    );
    let mut hash = FNV_OFFSET_BASIS;
    for byte in identity.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{WORKSPACE_ID_PREFIX}{hash:016x}")
}

fn git_remote(root: &Path) -> (Option<String>, WorkspaceSnapshotFieldState) {
    let Ok(output) = non_interactive_command(GIT_EXECUTABLE)
        .args(REMOTE_ORIGIN_ARGUMENTS)
        .current_dir(root)
        .output()
    else {
        return (None, WorkspaceSnapshotFieldState::Failed);
    };
    if !output.status.success() {
        let state = if output.status.code() == Some(GIT_VALUE_ABSENT_EXIT_CODE) {
            WorkspaceSnapshotFieldState::Absent
        } else {
            WorkspaceSnapshotFieldState::Failed
        };
        return (None, state);
    }
    let Ok(value) = String::from_utf8(output.stdout) else {
        return (None, WorkspaceSnapshotFieldState::Failed);
    };
    let value = value.trim().to_owned();
    if value.is_empty() {
        (None, WorkspaceSnapshotFieldState::Absent)
    } else {
        (Some(value), WorkspaceSnapshotFieldState::Collected)
    }
}

fn git_status(
    root: &Path,
) -> (
    Option<String>,
    Option<bool>,
    WorkspaceSnapshotFieldState,
    WorkspaceSnapshotFieldState,
) {
    let Ok(output) = non_interactive_command(GIT_EXECUTABLE)
        .args(GIT_STATUS_ARGUMENTS)
        .current_dir(root)
        .output()
    else {
        return (
            None,
            None,
            WorkspaceSnapshotFieldState::Failed,
            WorkspaceSnapshotFieldState::Failed,
        );
    };
    if !output.status.success() {
        return (
            None,
            None,
            WorkspaceSnapshotFieldState::Failed,
            WorkspaceSnapshotFieldState::Failed,
        );
    }
    let Ok(value) = String::from_utf8(output.stdout) else {
        return (
            None,
            None,
            WorkspaceSnapshotFieldState::Failed,
            WorkspaceSnapshotFieldState::Failed,
        );
    };
    let mut lines = value.lines();
    let Some(branch_line) = lines.next() else {
        return (
            None,
            None,
            WorkspaceSnapshotFieldState::Failed,
            WorkspaceSnapshotFieldState::Failed,
        );
    };
    let branch = parse_git_branch(branch_line);
    let branch_state = if branch.is_some() {
        WorkspaceSnapshotFieldState::Collected
    } else {
        WorkspaceSnapshotFieldState::Absent
    };
    (
        branch,
        Some(lines.any(|line| !line.trim().is_empty())),
        branch_state,
        WorkspaceSnapshotFieldState::Collected,
    )
}

fn parse_git_branch(line: &str) -> Option<String> {
    let rest = line.strip_prefix(GIT_BRANCH_LINE_PREFIX)?;
    let name = rest
        .strip_prefix(GIT_UNBORN_BRANCH_PREFIX)
        .unwrap_or(rest)
        .split(GIT_UPSTREAM_SEPARATOR)
        .next()?;
    let branch = name.split(' ').next()?;
    (branch != GIT_DETACHED_BRANCH && !branch.is_empty()).then(|| branch.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_NOW_MS: u64 = 42;

    #[test]
    fn nested_cwd_resolves_to_the_same_workspace() {
        let root = axon_test_support::test_temp_dir();
        let nested = root.path().join("app").join("feature");
        std::fs::create_dir_all(root.path().join(GIT_DIRECTORY_NAME)).expect("Git 根目录");
        std::fs::create_dir_all(&nested).expect("嵌套目录");
        let resolver = WorkspaceResolver;
        let root_workspace = resolver
            .resolve(root.path(), TEST_NOW_MS)
            .expect("根目录解析");
        let nested_workspace = resolver
            .resolve(&nested, TEST_NOW_MS)
            .expect("嵌套目录解析");

        assert_eq!(root_workspace.workspace_id, nested_workspace.workspace_id);
        assert_eq!(
            root_workspace.workspace_root,
            nested_workspace.workspace_root
        );
    }

    #[test]
    fn git_snapshot_collects_remote_branch_and_dirty_with_one_probe_each() {
        let root = axon_test_support::test_temp_dir();
        run_git(root.path(), &["init", "-q"]);
        run_git(root.path(), &["config", "user.email", "axon@example.test"]);
        run_git(root.path(), &["config", "user.name", "Axon Test"]);
        run_git(
            root.path(),
            &["remote", "add", "origin", "https://example.test/axon.git"],
        );
        std::fs::write(root.path().join("README.md"), "初始内容").expect("写入 fixture");
        run_git(root.path(), &["add", "README.md"]);
        run_git(root.path(), &["commit", "-qm", "fixture"]);
        std::fs::write(root.path().join("README.md"), "修改内容").expect("修改 fixture");

        let snapshot = WorkspaceResolver
            .resolve_snapshot(root.path(), TEST_NOW_MS)
            .expect("解析 Workspace 快照");

        assert_eq!(
            snapshot.git.remote.as_deref(),
            Some("https://example.test/axon.git")
        );
        assert!(snapshot.git.branch.is_some());
        assert_eq!(snapshot.git.dirty, Some(true));
        assert_eq!(
            snapshot.git.remote_state,
            WorkspaceSnapshotFieldState::Collected
        );
        assert_eq!(
            snapshot.git.branch_state,
            WorkspaceSnapshotFieldState::Collected
        );
        assert_eq!(
            snapshot.git.dirty_state,
            WorkspaceSnapshotFieldState::Collected
        );
        assert_eq!(snapshot.git.remote_probe_count, 1);
        assert_eq!(snapshot.git.status_probe_count, 1);
    }

    #[test]
    fn detached_head_and_missing_remote_are_explicitly_absent() {
        let root = axon_test_support::test_temp_dir();
        run_git(root.path(), &["init", "-q"]);
        run_git(root.path(), &["config", "user.email", "axon@example.test"]);
        run_git(root.path(), &["config", "user.name", "Axon Test"]);
        std::fs::write(root.path().join("README.md"), "初始内容").expect("写入 fixture");
        run_git(root.path(), &["add", "README.md"]);
        run_git(root.path(), &["commit", "-qm", "fixture"]);
        run_git(root.path(), &["checkout", "--detach", "-q"]);

        let snapshot = WorkspaceResolver
            .resolve_snapshot(root.path(), TEST_NOW_MS)
            .expect("解析 detached Workspace 快照");

        assert!(snapshot.git.remote.is_none());
        assert!(snapshot.git.branch.is_none());
        assert_eq!(
            snapshot.git.remote_state,
            WorkspaceSnapshotFieldState::Absent
        );
        assert_eq!(
            snapshot.git.branch_state,
            WorkspaceSnapshotFieldState::Absent
        );
        assert_eq!(snapshot.git.dirty, Some(false));
    }

    #[test]
    fn non_git_workspace_skips_all_git_probes() {
        let root = axon_test_support::test_temp_dir();

        let snapshot = WorkspaceResolver
            .resolve_snapshot(root.path(), TEST_NOW_MS)
            .expect("解析非 Git Workspace 快照");

        assert_eq!(snapshot.git.remote_probe_count, 0);
        assert_eq!(snapshot.git.status_probe_count, 0);
        assert_eq!(
            snapshot.git.remote_state,
            WorkspaceSnapshotFieldState::NotApplicable
        );
        assert_eq!(
            snapshot.git.branch_state,
            WorkspaceSnapshotFieldState::NotApplicable
        );
        assert_eq!(
            snapshot.git.dirty_state,
            WorkspaceSnapshotFieldState::NotApplicable
        );
    }

    #[test]
    fn invalid_git_marker_reports_failed_fields() {
        let root = axon_test_support::test_temp_dir();
        std::fs::create_dir(root.path().join(GIT_DIRECTORY_NAME)).expect("创建无效 Git 标记");

        let snapshot = WorkspaceResolver
            .resolve_snapshot(root.path(), TEST_NOW_MS)
            .expect("Workspace 根路径仍应可解析");

        assert_eq!(
            snapshot.git.remote_state,
            WorkspaceSnapshotFieldState::Failed
        );
        assert_eq!(
            snapshot.git.branch_state,
            WorkspaceSnapshotFieldState::Failed
        );
        assert_eq!(
            snapshot.git.dirty_state,
            WorkspaceSnapshotFieldState::Failed
        );
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let status = non_interactive_command(GIT_EXECUTABLE)
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("Git 命令可执行");
        assert!(status.success(), "Git fixture 命令失败：{args:?}");
    }

    #[test]
    fn live_clone_keeps_an_independent_workspace_identity() {
        let active_root = axon_test_support::test_temp_dir();
        let candidate = workspace("candidate", "E:/clone-new", TEST_NOW_MS);
        let existing = workspace("existing", active_root.path().to_string_lossy().as_ref(), 1);

        let resolved =
            WorkspaceResolver.reconcile_alias(candidate.clone(), std::slice::from_ref(&existing));

        assert_eq!(resolved, candidate);
    }

    #[test]
    fn missing_clone_rebinds_known_workspace_and_preserves_alias() {
        let missing_root = axon_test_support::test_temp_root().join("axon-missing-workspace-root");
        let candidate = workspace("candidate", "E:/clone-new", TEST_NOW_MS);
        let existing = workspace("existing", missing_root.to_string_lossy().as_ref(), 1);

        let resolved =
            WorkspaceResolver.reconcile_alias(candidate, std::slice::from_ref(&existing));

        assert_eq!(resolved.workspace_id, existing.workspace_id);
        assert_eq!(resolved.workspace_root, "E:/clone-new");
        assert_eq!(resolved.aliases, [existing.workspace_root]);
    }

    fn workspace(id_suffix: &str, root: &str, now_ms: u64) -> WorkspaceRecord {
        WorkspaceRecord {
            workspace_id: format!("workspace-{id_suffix}"),
            display_name: id_suffix.to_owned(),
            workspace_root: root.to_owned(),
            git_remote: Some("https://example.test/axon.git".to_owned()),
            default_branch: Some("main".to_owned()),
            aliases: Vec::new(),
            memory_namespace: format!("{MEMORY_NAMESPACE_PREFIX}workspace-{id_suffix}"),
            pinned: false,
            last_seen_at_ms: now_ms,
        }
    }
}
