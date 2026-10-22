//! shell_command 的 argv 形态预检。
//!
//! 本模块**只校验 argv 结构**（对齐 DSH）：`bash -lc <script>`、`pwsh -NoLogo -NoProfile
//! -NonInteractive -Command <script>` 以及 WSL 的 9 元 argv 是否等于 Axon 规范化生成的
//! 形态。脚本内容完全不做预检：heredoc、here-string、多行结构一律放行，语法与方言错误
//! 交给 shell 自身在运行期回报，而不是由词法分析猜测。
//!
//! 非交互保障由运行时结构提供：
//! 1. argv 固定为无 profile 的显式非交互形态（`-NoLogo -NoProfile -NonInteractive -Command`）；
//! 2. 每次执行注入 `PAGER=cat`、`GIT_PAGER=cat`，非 TTY 追加 `NO_COLOR=1`、`TERM=dumb`；
//! 3. 会话带 `timeout_ms` 截止时间与空闲回收，挂起的命令不会无限占用会话；
//! 4. 真正需要交互时走 `tty: true` 的 PTY 通道，并用 `builtin.write_stdin` 续接。

use axon_domain::{AxonError, AxonResult};

use crate::builtin_shell_script::is_canonical_wsl_script_command;

pub(crate) const SHELL_STATIC_ANALYSIS_SHELL_FLAG_ERROR: &str =
    "shell_command 静态检查失败：shell wrapper 参数不符合平台规范";

const POWERSHELL_ARGS: &[&str] = &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"];
const POWERSHELL_TTY_ARGS: &[&str] = &["-NoLogo", "-NoProfile", "-Command"];
const POSIX_COMMAND_FLAGS: &[&str] = &["-c", "-lc"];

pub(crate) struct ShellCommandStaticAnalyzer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StaticShellDialect {
    Posix,
    PowerShell,
}

impl ShellCommandStaticAnalyzer {
    /// 校验 argv 是否为 Axon 规范化生成的 wrapper 形态。
    ///
    /// 只查**结构**：脚本内容（方言、多行闭合、危险词）一律不进入本判定。
    pub(crate) fn validate_command(command: &[String]) -> AxonResult<()> {
        let Some((program, args)) = command.split_first() else {
            return Ok(());
        };
        let Some(shell_name) = shell_program_name(program) else {
            return Ok(());
        };
        let dialect = match shell_name.as_str() {
            "bash" | "zsh" | "sh" => Some(StaticShellDialect::Posix),
            "pwsh" | "powershell" => Some(StaticShellDialect::PowerShell),
            "wsl" => {
                if !is_canonical_wsl_script_command(command) {
                    return Err(AxonError::tool_invalid_arguments(
                        SHELL_STATIC_ANALYSIS_SHELL_FLAG_ERROR,
                    ));
                }
                None
            }
            _ => None,
        };
        if let Some(dialect) = dialect {
            validate_wrapper_shape(dialect, args)?;
        }
        Ok(())
    }
}

fn validate_wrapper_shape(dialect: StaticShellDialect, args: &[String]) -> AxonResult<()> {
    let valid = match dialect {
        StaticShellDialect::Posix => {
            args.len() == 2
                && args
                    .first()
                    .is_some_and(|flag| POSIX_COMMAND_FLAGS.contains(&flag.as_str()))
        }
        StaticShellDialect::PowerShell => {
            args.len() == POWERSHELL_ARGS.len() + 1
                && args[..POWERSHELL_ARGS.len()]
                    .iter()
                    .map(String::as_str)
                    .eq(POWERSHELL_ARGS.iter().copied())
                || args.len() == POWERSHELL_TTY_ARGS.len() + 1
                    && args[..POWERSHELL_TTY_ARGS.len()]
                        .iter()
                        .map(String::as_str)
                        .eq(POWERSHELL_TTY_ARGS.iter().copied())
        }
    };
    if valid {
        Ok(())
    } else {
        Err(AxonError::tool_invalid_arguments(
            SHELL_STATIC_ANALYSIS_SHELL_FLAG_ERROR,
        ))
    }
}

pub(crate) fn shell_program_name(program: &str) -> Option<String> {
    std::path::Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_ascii_lowercase())
}

pub(crate) use crate::shell_command_wrapper_unwrapper::unwrap_outer_shell_wrapper;

#[cfg(test)]
mod tests {
    use super::*;

    fn command_of(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn accepts_canonical_shell_wrapper_shapes() {
        for parts in [
            &["bash", "-lc", "pwd; ls"][..],
            &["sh", "-c", "echo axon"][..],
            &[
                "pwsh",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-Date",
            ][..],
            // tty 通道省略 `-NonInteractive`，是第二种规范化形态。
            &["pwsh", "-NoLogo", "-NoProfile", "-Command", "Get-Date"][..],
            &[
                "powershell",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-Date",
            ][..],
        ] {
            ShellCommandStaticAnalyzer::validate_command(&command_of(parts))
                .expect("Axon 规范化生成的 wrapper 形态必须被接受");
        }
    }

    #[test]
    fn rejects_wrapper_shapes_outside_the_platform_contract() {
        for parts in [
            &["bash", "-x", "pwd"][..],
            &["pwsh", "-Command", "Get-Date"][..],
            &[
                "pwsh",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
            ][..],
        ] {
            let error =
                ShellCommandStaticAnalyzer::validate_command(&command_of(parts)).unwrap_err();

            assert!(matches!(
                error,
                AxonError::Tool(axon_domain::ToolError::InvalidArguments(reason))
                    if reason == SHELL_STATIC_ANALYSIS_SHELL_FLAG_ERROR
            ));
        }
    }

    /// 对齐 DSH 的回归：脚本内容不进入任何预检通道。
    ///
    /// 修复前 PowerShell 方言拒 Bash heredoc、POSIX 方言拒 here-string，两条修法指引互相
    /// 指向对方，Windows 上两种多行写法同时被禁。现在一律放行，由 shell 自身裁决。
    #[test]
    fn script_content_is_never_inspected() {
        for command in [
            command_of(&[
                "pwsh",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "python - <<'PY'\nprint('ok')\nPY",
            ]),
            command_of(&["bash", "-lc", "@'\nprint('ok')\n'@ | python3 -"]),
            command_of(&[
                "pwsh",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Remove-Item -Recurse -Force C:\\",
            ]),
        ] {
            assert!(
                ShellCommandStaticAnalyzer::validate_command(&command).is_ok(),
                "脚本文本不得参与预检：{command:?}"
            );
        }
    }
}
