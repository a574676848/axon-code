mod mcp;
mod skills;

use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) use mcp::execute_runtime_mcp_tool_call;
pub(crate) use mcp::{
    RuntimeExecutionToolSnapshot, build_execution_tool_registry_for_session,
    build_runtime_tool_registry_snapshot, build_runtime_tool_registry_snapshot_with_refresh,
    execute_runtime_mcp_list_resources, execute_runtime_mcp_read_resource,
    execute_runtime_mcp_tool_call_with_cancel, runtime_tool_registry_or_builtin,
    try_build_runtime_tool_registry_snapshot,
};
pub(crate) use skills::{
    RuntimeSkillsSnapshot, build_runtime_skills_manager, execute_runtime_skill_invocation,
    load_runtime_skills_snapshot,
};

pub(crate) fn runtime_home() -> Option<std::path::PathBuf> {
    // Daemon 由服务账户运行时，用户级 Skills 与 MCP 配置必须跟随运行态根目录。
    axon_config::user_home_dir().ok()
}

/// 解析本轮实际 Workspace；Home 目录树内不自动选择 Workspace。
pub(crate) fn resolve_workspace_cwd(
    config_path: &Path,
    cwd: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let Some(cwd) = cwd else {
        return Ok(None);
    };
    let cwd = cwd
        .canonicalize()
        .map_err(|_| format!("工作目录不可访问：{}", cwd.display()))?;
    let home = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())?;
    let home = home.canonicalize().unwrap_or(home);
    Ok((!cwd.starts_with(home)).then_some(cwd))
}

#[cfg(test)]
mod tests {
    use super::resolve_workspace_cwd;
    use std::fs;

    #[test]
    fn home_tree_has_no_default_workspace_and_external_cwd_is_canonical() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-runtime-workspace-resolution-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        let home = root.join("home");
        let home_child = home.join("data");
        let workspace = root.join("workspace");
        let workspace_alias = workspace.join("nested").join("..");
        let config_path = home.join("config.toml");
        fs::create_dir_all(&home_child).unwrap();
        fs::create_dir_all(workspace.join("nested")).unwrap();

        assert_eq!(
            resolve_workspace_cwd(&config_path, Some(&home_child)).unwrap(),
            None
        );
        assert_eq!(
            resolve_workspace_cwd(&config_path, Some(&workspace_alias)).unwrap(),
            Some(workspace.canonicalize().unwrap())
        );
        let _ = fs::remove_dir_all(root);
    }
}
