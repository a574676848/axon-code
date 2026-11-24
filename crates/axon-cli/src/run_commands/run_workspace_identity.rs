use axon_agent_runtime::AgentPaneGitState;
use axon_session::{WorkspaceRecord, WorkspaceResolver, WorkspaceSnapshot};

pub(crate) fn workspace_record(
    path: &std::path::Path,
    now_ms: u64,
) -> Result<WorkspaceRecord, String> {
    WorkspaceResolver
        .resolve(path, now_ms)
        .map_err(|error| error.to_string())
}

/// 将本 turn 已采集的 Workspace Git 事实投影到 Agent pane。
pub(crate) fn workspace_git_state(
    snapshot: Option<&WorkspaceSnapshot>,
) -> Option<AgentPaneGitState> {
    let git = &snapshot?.git;
    Some(AgentPaneGitState {
        branch: git.branch.clone()?,
        dirty: git.dirty?,
    })
}

#[cfg(test)]
mod tests {
    use super::workspace_git_state;
    use axon_session::WorkspaceResolver;

    #[test]
    fn non_git_workspace_has_no_git_state() {
        let root = axon_test_support::test_temp_dir();
        let snapshot = WorkspaceResolver
            .resolve_snapshot(root.path(), 1)
            .expect("解析 Workspace");
        assert!(workspace_git_state(Some(&snapshot)).is_none());
    }

    #[test]
    fn git_workspace_projects_branch_and_dirty_state() {
        let root = axon_test_support::test_temp_dir();
        run_git(root.path(), &["init", "-q"]);
        run_git(root.path(), &["config", "user.email", "axon@example.test"]);
        run_git(root.path(), &["config", "user.name", "Axon Test"]);
        std::fs::write(root.path().join("README.md"), "初始内容").expect("写入 fixture");
        run_git(root.path(), &["add", "README.md"]);
        run_git(root.path(), &["commit", "-qm", "fixture"]);

        let clean_snapshot = WorkspaceResolver
            .resolve_snapshot(root.path(), 1)
            .expect("解析 clean Workspace");
        let clean = workspace_git_state(Some(&clean_snapshot)).expect("Git 工作区应有状态");
        assert!(!clean.branch.is_empty());
        assert!(!clean.dirty);

        std::fs::write(root.path().join("README.md"), "修改内容").expect("修改 fixture");
        let dirty_snapshot = WorkspaceResolver
            .resolve_snapshot(root.path(), 2)
            .expect("解析 dirty Workspace");
        let dirty = workspace_git_state(Some(&dirty_snapshot)).expect("Git 工作区应有状态");
        assert_eq!(dirty.branch, clean.branch);
        assert!(dirty.dirty);
    }

    fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let status = axon_process::non_interactive_command("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("Git 命令可执行");
        assert!(status.success(), "Git fixture 命令失败: {args:?}");
    }
}
