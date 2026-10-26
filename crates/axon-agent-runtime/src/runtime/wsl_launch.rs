//! WSL 启动面：UNC 工作目录解析与 wsl.exe 命令行组装。
//!
//! 从 `builtin_shell_script` 拆出：这一组只在 Windows 上生效，且只关心「\\wsl$ 形态的
//! 工作目录怎么变成 Linux 路径、argv 怎么拼」，与其它方言的映射互不干扰。

use super::*;
// 父模块的私有常量不会被 `use super::*` 带出，按本仓既有做法显式引入。
use super::{
    WSL_BASH_PROGRAM, WSL_CANONICAL_LEGACY_PREFIX, WSL_CANONICAL_LOCALHOST_PREFIX, WSL_CWD_ARG,
    WSL_DISTRIBUTION_ARG, WSL_EXEC_ARG, WSL_LEGACY_PREFIX, WSL_LOCALHOST_PREFIX, WSL_PROGRAM,
    valid_wsl_distribution,
};
use axon_domain::AxonResult;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(windows)]
pub(super) struct WslWorkingDirectory {
    pub(super) distribution: String,
    pub(super) linux_path: String,
}

#[cfg(windows)]
pub(super) fn parse_wsl_unc_path(path: &str) -> AxonResult<Option<WslWorkingDirectory>> {
    let Some(remainder) = strip_ascii_case_prefix(path, WSL_CANONICAL_LOCALHOST_PREFIX)
        .or_else(|| strip_ascii_case_prefix(path, WSL_CANONICAL_LEGACY_PREFIX))
        .or_else(|| strip_ascii_case_prefix(path, WSL_LOCALHOST_PREFIX))
        .or_else(|| strip_ascii_case_prefix(path, WSL_LEGACY_PREFIX))
    else {
        return Ok(None);
    };
    let mut components = remainder
        .split('\\')
        .filter(|component| !component.is_empty());
    let distribution = components.next().unwrap_or_default();
    if !valid_wsl_distribution(distribution) {
        return Err(AxonError::tool_invalid_arguments(
            "WSL UNC 路径中的发行版名称无效",
        ));
    }
    let linux_path = format!("/{}", components.collect::<Vec<_>>().join("/"));
    Ok(Some(WslWorkingDirectory {
        distribution: distribution.to_owned(),
        linux_path,
    }))
}

#[cfg(windows)]
pub(super) fn strip_ascii_case_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .and_then(|_| value.get(prefix.len()..))
}

/// 组装 WSL 侧 argv：Linux 端固定是 `/bin/bash -lc <script>`。
///
/// 该段 argv 由 `shell_command_argv` 的 `PosixLogin` 分支生成，保证 WSL 形态与其它方言
/// 共用同一处映射；分发版名与 Linux 工作目录只在前 6 个元素上附加。
#[cfg(windows)]
pub(super) fn wsl_command_argv(
    cwd: &WslWorkingDirectory,
    script: &str,
    tty: bool,
) -> AxonResult<Vec<String>> {
    let login = ResolvedShell {
        dialect: ShellDialect::PosixLogin,
        program: WSL_BASH_PROGRAM.to_owned(),
    };
    let mut command = vec![
        WSL_PROGRAM.to_owned(),
        WSL_DISTRIBUTION_ARG.to_owned(),
        cwd.distribution.clone(),
        WSL_CWD_ARG.to_owned(),
        cwd.linux_path.clone(),
        WSL_EXEC_ARG.to_owned(),
    ];
    command.extend(shell_command_argv(&login, script, tty)?);
    Ok(command)
}
