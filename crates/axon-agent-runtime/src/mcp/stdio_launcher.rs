//! MCP stdio 子进程启动边界。
//!
//! 这里集中处理 Codex 同类实现里的两个关键语义：Windows wrapper 解析与受控环境注入。

use std::{
    collections::BTreeMap,
    env, io,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use axon_domain::{AxonError, AxonResult};

use axon_process::{
    configure_non_interactive_process,
    resource_fence::{
        ProcessFenceHandle, ProcessResourceLimits, configure_process_resource_limits,
        fence_spawned_child,
    },
};

use crate::{McpServerCommand, command_resolver};

const WINDOWS_WRAPPER_EXTENSIONS: [&str; 3] = ["cmd", "exe", "bat"];

#[cfg(windows)]
const DEFAULT_ENV_VARS: &[&str] = &[
    "PATH",
    "PATHEXT",
    "RUBYOPT",
    "SHELL",
    "COMSPEC",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "USERNAME",
    "USERDOMAIN",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "PROGRAMDATA",
    "LOCALAPPDATA",
    "APPDATA",
    "TEMP",
    "TMP",
    "TMPDIR",
    "POWERSHELL",
    "PWSH",
];

#[cfg(not(windows))]
const DEFAULT_ENV_VARS: &[&str] = &[
    "HOME", "LOGNAME", "PATH", "RUBYOPT", "SHELL", "USER", "LANG", "LC_ALL", "LC_CTYPE", "TERM",
    "TMPDIR", "TZ",
];

/// 受平台围栏保护的 MCP stdio 子进程句柄。
///
/// 围栏句柄必须与子进程同生命周期：Windows 上是带 `KILL_ON_JOB_CLOSE` 的 Job Object，
/// Linux 上是 `PR_SET_PDEATHSIG`。宿主进程退出（正常退出、`std::process::exit` 或崩溃）时
/// 由内核回收 MCP 子进程。
///
/// 缺少这层保护会留下孤儿 MCP server：它持有宿主 std 句柄的副本，使任何按管道捕获宿主
/// 输出的调用（脚本、CI、Agent 工具链）永远等不到 EOF 而表现为挂起，同时持续占用本机
/// 进程与端口。
#[derive(Debug)]
pub struct McpStdioChild {
    child: Child,
    /// 与子进程同生命周期的围栏句柄；`None` 表示本平台未绑定成功（尽力而为）。
    _fence: Option<ProcessFenceHandle>,
}

impl McpStdioChild {
    /// 是否已绑定平台围栏。
    #[must_use]
    pub const fn is_fenced(&self) -> bool {
        self._fence.is_some()
    }

    /// 等待子进程结束并收集输出，围栏句柄在等待期间保持有效。
    ///
    /// # Errors
    ///
    /// 与 [`Child::wait_with_output`] 相同：等待或读取输出失败时返回错误。
    pub fn wait_with_output(self) -> io::Result<std::process::Output> {
        self.child.wait_with_output()
    }
}

impl Deref for McpStdioChild {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.child
    }
}

impl DerefMut for McpStdioChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

/// 启动本机 MCP stdio 子进程。
///
/// # Errors
///
/// 程序解析、工作目录解析或进程启动失败时返回插件定义错误。
pub fn spawn_mcp_stdio_child(command: &McpServerCommand) -> AxonResult<McpStdioChild> {
    let mut process = mcp_stdio_process(command)?;
    // 围栏必须在 spawn 之前配置：Linux 走 pre_exec，Windows 走启动后的 Job Object 绑定。
    configure_process_resource_limits(&mut process, mcp_stdio_fence_limits());
    let child = process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AxonError::runtime_failure(format_spawn_error(command, &error)))?;
    // 尽力而为：围栏绑定失败只意味着少一层孤儿保护，不能因此拒绝已启动的 MCP server。
    let fence = fence_spawned_child(&child, mcp_stdio_fence_limits()).unwrap_or(None);
    Ok(McpStdioChild {
        child,
        _fence: fence,
    })
}

/// MCP stdio 子进程围栏限额。
///
/// 只补孤儿回收，不施加未经 MCP server 验证的资源上限：MCP server 是第三方程序
/// （IDE、Node 工具链等），其正常负载不在本仓的限额标定范围内。
fn mcp_stdio_fence_limits() -> ProcessResourceLimits {
    ProcessResourceLimits::off()
}

fn mcp_stdio_process(command: &McpServerCommand) -> AxonResult<Command> {
    let cwd = command
        .working_dir
        .as_deref()
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(env::current_dir)
        .map_err(|error| {
            AxonError::runtime_failure(format!(
                "MCP stdio 工作目录读取失败：cwd={} error_kind={:?} os_error={} error={}",
                command.working_dir.as_deref().unwrap_or("<current_dir>"),
                error.kind(),
                error
                    .raw_os_error()
                    .map_or_else(|| "none".to_owned(), |value| value.to_string()),
                error
            ))
        })?;
    let envs = mcp_stdio_environment(&command.env);
    let resolved_program = resolve_mcp_stdio_program(&command.program, &cwd);

    let mut process = Command::new(resolved_program);
    configure_non_interactive_process(&mut process);
    process.args(&command.args).env_clear().envs(&envs);
    if command.working_dir.is_some() {
        process.current_dir(cwd);
    }
    Ok(process)
}

fn format_spawn_error(command: &McpServerCommand, error: &io::Error) -> String {
    let working_dir = command.working_dir.as_deref().unwrap_or("<current_dir>");
    format!(
        "MCP stdio server 启动失败：program={} args_count={} working_dir={} cwd={} error_kind={:?} os_error={} error={}",
        command.program,
        command.args.len(),
        working_dir,
        env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "<unavailable>".to_owned()),
        error.kind(),
        error
            .raw_os_error()
            .map_or_else(|| "none".to_owned(), |value| value.to_string()),
        error
    )
}

fn mcp_stdio_environment(overrides: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    mcp_stdio_environment_from(env::vars(), overrides)
}

fn mcp_stdio_environment_from(
    env_vars: impl IntoIterator<Item = (String, String)>,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut values = env_vars
        .into_iter()
        .filter(|(key, _)| {
            DEFAULT_ENV_VARS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(key))
        })
        .collect::<BTreeMap<_, _>>();
    values
        .entry("LANG".to_owned())
        .or_insert_with(|| "en_US.UTF-8".to_owned());
    values
        .entry("RUBYOPT".to_owned())
        .or_insert_with(|| "-EUTF-8:UTF-8".to_owned());
    values.extend(overrides.clone());
    values
}

fn resolve_mcp_stdio_program(program: &str, cwd: &Path) -> PathBuf {
    let direct = PathBuf::from(program);
    if cfg!(windows)
        && direct.extension().is_none()
        && let Some(wrapper) = windows_wrapper_candidate(&direct, cwd)
    {
        return wrapper;
    }
    command_resolver::command_path(program).unwrap_or(direct)
}

fn windows_wrapper_candidate(program: &Path, cwd: &Path) -> Option<PathBuf> {
    let base = if program.is_absolute() {
        program.to_path_buf()
    } else {
        cwd.join(program)
    };
    WINDOWS_WRAPPER_EXTENSIONS
        .into_iter()
        .map(|extension| base.with_extension(extension))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_environment_keeps_core_vars_and_applies_overrides() {
        let envs = mcp_stdio_environment_from(
            [
                ("PATH".to_owned(), "/usr/bin".to_owned()),
                ("LANG".to_owned(), "zh_CN.UTF-8".to_owned()),
            ],
            &BTreeMap::from([("AXON_MCP_TEST_VALUE".to_owned(), "ready".to_owned())]),
        );

        assert_eq!(
            envs.get("AXON_MCP_TEST_VALUE").map(String::as_str),
            Some("ready")
        );
        assert!(
            envs.get("LANG")
                .is_some_and(|value| value.to_ascii_uppercase().contains("UTF-8"))
        );
        assert_eq!(
            envs.get("RUBYOPT").map(String::as_str),
            Some("-EUTF-8:UTF-8")
        );
        assert!(envs.keys().any(|key| key.eq_ignore_ascii_case("PATH")));
    }

    #[test]
    fn stdio_environment_supplies_utf8_defaults() {
        let envs = mcp_stdio_environment_from([], &BTreeMap::new());

        assert_eq!(envs.get("LANG").map(String::as_str), Some("en_US.UTF-8"));
        assert_eq!(
            envs.get("RUBYOPT").map(String::as_str),
            Some("-EUTF-8:UTF-8")
        );
    }

    #[test]
    fn stdio_environment_allows_utf8_defaults_to_be_overridden() {
        let envs = mcp_stdio_environment_from(
            [],
            &BTreeMap::from([
                ("LANG".to_owned(), "C".to_owned()),
                ("RUBYOPT".to_owned(), "--disable=gems".to_owned()),
            ]),
        );

        assert_eq!(envs.get("LANG").map(String::as_str), Some("C"));
        assert_eq!(
            envs.get("RUBYOPT").map(String::as_str),
            Some("--disable=gems")
        );
    }

    #[test]
    fn spawn_error_includes_os_context_without_environment_values() {
        let command = McpServerCommand {
            program: "axon-mcp-command-that-does-not-exist".to_owned(),
            args: vec!["--diagnostic".to_owned()],
            working_dir: None,
            env: BTreeMap::from([("MCP_SECRET".to_owned(), "should-not-appear".to_owned())]),
            tool_timeout_ms: None,
        };

        let error = spawn_mcp_stdio_child(&command).expect_err("不存在的 MCP 命令应启动失败");
        let text = format!("{error:?}");
        assert!(text.contains("error_kind=NotFound"));
        assert!(text.contains("args_count=1"));
        assert!(!text.contains("should-not-appear"));
    }

    #[test]
    #[cfg(windows)]
    fn stdio_child_is_bound_to_kill_on_close_job() {
        let command = McpServerCommand {
            program: "cmd".to_owned(),
            args: vec![
                "/d".to_owned(),
                "/s".to_owned(),
                "/c".to_owned(),
                "ping -n 20 127.0.0.1 > nul".to_owned(),
            ],
            working_dir: None,
            env: BTreeMap::new(),
            tool_timeout_ms: None,
        };

        let mut child = spawn_mcp_stdio_child(&command).expect("长生命周期 MCP 子进程应可启动");

        assert!(
            child.is_fenced(),
            "MCP 子进程必须绑定 KILL_ON_JOB_CLOSE 作业对象，否则宿主退出后会留下孤儿并占住继承的 stdout"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    #[cfg(windows)]
    fn windows_resolver_prefers_cmd_wrapper_for_extensionless_path() {
        let temp = axon_test_support::test_temp_dir();
        let base = temp.path().join("axon-mcp-wrapper");
        std::fs::write(&base, "not executable").expect("无扩展 shim 应可写入");
        std::fs::write(base.with_extension("cmd"), "@echo off\r\nexit /b 0\r\n")
            .expect("cmd wrapper 应可写入");

        let resolved = resolve_mcp_stdio_program(&base.to_string_lossy(), temp.path());

        assert_eq!(resolved, base.with_extension("cmd"));
    }
}
