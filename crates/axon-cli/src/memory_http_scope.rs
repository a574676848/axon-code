//! 记忆管理 Gateway 的作用域解析。
//!
//! 管理页写入必须落在与 Agent 完全相同的 scope 上：Personal 走 owner 派生的
//! `personal:<owner>` namespace，Workspace 走「必填文件夹目录 → Workspace Registry
//! namespace」的派生链。任何一侧偏移都会让页面读写到另一个桶，表现为“有记忆却看不到”。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axon_agent_runtime::{MemoryToolScope, memory_scope_refs, memory_scope_refs_with_namespace};
use axon_domain::MemoryKind;
use axon_session::WorkspaceStore;

/// 记忆管理作用域错误：区分客户端提交错误与服务端内部失败。
pub(crate) enum MemoryScopeError {
    /// 客户端提交不合法。
    BadRequest(String),
    /// 服务端内部失败。
    Internal(String),
}

/// Personal 五值类型只允许这两个。
#[must_use]
pub(crate) const fn is_personal_kind(kind: MemoryKind) -> bool {
    matches!(kind, MemoryKind::Profile | MemoryKind::Preference)
}

/// 解析 Personal scope。
pub(crate) fn personal_scope(axon_root: &Path) -> Result<MemoryToolScope, MemoryScopeError> {
    let owner = crate::runtime_principal_context::load_or_provision_owner_id(axon_root)
        .map_err(MemoryScopeError::Internal)?;
    Ok(MemoryToolScope::Personal { owner_id: owner })
}

/// 由必填文件夹目录派生 Workspace scope。
pub(crate) fn workspace_scope(
    config_path: &Path,
    workspace_path: &str,
) -> Result<MemoryToolScope, MemoryScopeError> {
    let trimmed = workspace_path.trim();
    if trimmed.is_empty() {
        return Err(MemoryScopeError::BadRequest(
            "仓库记忆必须提供文件夹目录".to_owned(),
        ));
    }
    let cwd = PathBuf::from(trimmed);
    if !cwd.is_absolute() {
        return Err(MemoryScopeError::BadRequest(
            "文件夹目录必须是绝对路径".to_owned(),
        ));
    }
    if !cwd.is_dir() {
        return Err(MemoryScopeError::BadRequest(format!(
            "文件夹目录不可访问：{}",
            cwd.display()
        )));
    }
    // 与 Agent 写入链保持一致：Home 树内不注册 Workspace，其余走注册表 namespace。
    let namespace = match crate::runtime_tools::resolve_workspace_cwd(config_path, Some(&cwd))
        .map_err(MemoryScopeError::Internal)?
    {
        Some(workspace_root) => crate::run_commands::run_workspace_identity::workspace_record(
            &workspace_root,
            axon_time::unix_now_ms(),
        )
        .ok()
        .map(|workspace| workspace.memory_namespace),
        None => None,
    };
    Ok(MemoryToolScope::Workspace {
        scope: namespace.map_or_else(
            || memory_scope_refs(&cwd),
            |namespace_ref| memory_scope_refs_with_namespace(&cwd, &namespace_ref),
        ),
    })
}

/// 由类型与可选文件夹目录派生写入 scope。
pub(crate) fn scope_for_request(
    axon_root: &Path,
    config_path: &Path,
    kind: MemoryKind,
    workspace_path: Option<&str>,
) -> Result<MemoryToolScope, MemoryScopeError> {
    if is_personal_kind(kind) {
        if workspace_path.is_some_and(|value| !value.trim().is_empty()) {
            return Err(MemoryScopeError::BadRequest(
                "profile 与 preference 属于 Personal 记忆，不能提供文件夹目录".to_owned(),
            ));
        }
        return personal_scope(axon_root);
    }
    workspace_scope(config_path, workspace_path.unwrap_or_default())
}

/// 已注册 Workspace 的显示名索引：`memory_namespace` → 可读标签。
pub(crate) fn workspace_labels(axon_root: &Path) -> BTreeMap<String, String> {
    let Ok(store) = crate::workbench_http_adapter::runtime_store(axon_root).list_workspaces()
    else {
        return BTreeMap::new();
    };
    store
        .into_iter()
        .map(|workspace| {
            (
                workspace.memory_namespace,
                format!("{}（{}）", workspace.display_name, workspace.workspace_root),
            )
        })
        .collect()
}
