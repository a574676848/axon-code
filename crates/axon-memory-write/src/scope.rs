use std::path::Path;

use axon_crypto::sha256_tagged_ref;
use axon_memory::personal_memory_namespace;
use axon_process::non_interactive_command;

/// Memory 写入作用域。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWriteScope {
    /// workspace 与 branch 组合后的 namespace 引用。
    pub namespace_ref: String,
    /// repository 引用。
    pub repo_ref: Option<String>,
    /// user 引用。
    pub user_ref: Option<String>,
}

/// 从指定工作区派生写入 scope。
#[must_use]
pub fn memory_scope_refs(cwd: &Path) -> MemoryWriteScope {
    let workspace = sha256_tagged_ref("workspace", cwd.to_string_lossy().as_bytes());
    let branch_ref = git_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|branch| branch != "HEAD")
        .map(|branch| sha256_tagged_ref("branch", branch.as_bytes()));
    let repo_ref = git_output(cwd, &["config", "--get", "remote.origin.url"])
        .or_else(|| git_output(cwd, &["rev-parse", "--show-toplevel"]))
        .map(|repo| sha256_tagged_ref("repo", repo.as_bytes()));
    MemoryWriteScope {
        namespace_ref: branch_ref
            .map_or(workspace.clone(), |branch| format!("{workspace}:{branch}")),
        repo_ref,
        user_ref: None,
    }
}

/// 使用 Workspace Registry 分配的 namespace 构造 Workspace scope。
#[must_use]
pub fn memory_scope_refs_with_namespace(cwd: &Path, namespace_ref: &str) -> MemoryWriteScope {
    let mut scope = memory_scope_refs(cwd);
    if !namespace_ref.trim().is_empty() {
        scope.namespace_ref = namespace_ref.to_owned();
    }
    scope
}

/// 构造与读取侧一致的 Personal Memory scope。
#[must_use]
pub fn personal_memory_scope(user_ref: &str) -> MemoryWriteScope {
    if user_ref.trim().is_empty() {
        return MemoryWriteScope {
            namespace_ref: String::new(),
            repo_ref: None,
            user_ref: None,
        };
    }
    MemoryWriteScope {
        namespace_ref: personal_memory_namespace(user_ref),
        repo_ref: None,
        user_ref: Some(user_ref.to_owned()),
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = non_interactive_command("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_owner_cannot_construct_personal_scope() {
        assert!(personal_memory_scope(" ").user_ref.is_none());
    }
}
