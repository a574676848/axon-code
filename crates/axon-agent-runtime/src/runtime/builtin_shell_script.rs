//! shell_command 的跨平台默认 shell 脚本规范化。

use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;
use std::sync::OnceLock;

use axon_domain::{AxonError, AxonResult};

use axon_process::configure_non_interactive_process;

const PWSH_PROGRAM: &str = "pwsh";
#[cfg(windows)]
const WINDOWS_POWERSHELL_PROGRAM: &str = "powershell.exe";
#[cfg(windows)]
const WSL_PROGRAM: &str = "wsl.exe";
const WSL_DISTRIBUTION_ARG: &str = "--distribution";
const WSL_CWD_ARG: &str = "--cd";
const WSL_EXEC_ARG: &str = "--exec";
const WSL_BASH_PROGRAM: &str = "/bin/bash";
const WSL_BASH_COMMAND_ARG: &str = "-lc";
#[cfg(windows)]
const WSL_LOCALHOST_PREFIX: &str = r"\\wsl.localhost\";
#[cfg(windows)]
const WSL_LEGACY_PREFIX: &str = r"\\wsl$\";
#[cfg(windows)]
const WSL_CANONICAL_LOCALHOST_PREFIX: &str = r"\\?\UNC\wsl.localhost\";
#[cfg(windows)]
const WSL_CANONICAL_LEGACY_PREFIX: &str = r"\\?\UNC\wsl$\";
const PWSH_MAJOR_VERSION: u64 = 7;
const PWSH_VERSION_SCRIPT: &str = "$PSVersionTable.PSVersion.Major";
pub(crate) const PWSH_VERSION_ERROR: &str = "shell_command 的 pwsh 必须为 PowerShell 7+";
const WINDOWS_POWERSHELL_ERROR: &str = "shell_command 无法启动 Windows PowerShell";
pub(crate) const UNTRUSTED_SHELL_ERROR: &str =
    "shell_command 只允许 Axon 规范化生成的 shell 脚本命令";
pub(crate) const POWERSHELL_UTF8_PREFIX: &str = "try { $OutputEncoding=[Text.UTF8Encoding]::new($false); [Console]::InputEncoding=$OutputEncoding; [Console]::OutputEncoding=$OutputEncoding; if ($PSVersionTable.PSVersion.Major -ge 7) { $PSStyle.OutputRendering='PlainText' } } catch {}\n";
const POWERSHELL_PIPE_ARGS_PREFIX: &[&str] =
    &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"];
const POWERSHELL_TTY_ARGS_PREFIX: &[&str] = &["-NoLogo", "-NoProfile", "-Command"];
const POSIX_LOGIN_FLAG: &str = "-lc";
#[cfg(not(windows))]
const POSIX_COMMAND_FLAG: &str = "-c";
/// 禁用彩色输出：避免 ANSI 控制序列污染模型可见文本。
const NO_COLOR_ENV: (&str, &str) = ("NO_COLOR", "1");
/// 非 TTY 场景的降级终端类型：抑制依赖真实终端能力的输出形态。
const TERM_ENV: (&str, &str) = ("TERM", "dumb");
/// 禁用分页器：分页器在无交互输入时会一直等待，是命令挂起的直接成因。
const PAGER_ENV: (&str, &str) = ("PAGER", "cat");
/// 禁用 git 分页器：`git log`、`git diff` 会依据 TTY 自行启动分页器。
const GIT_PAGER_ENV: (&str, &str) = ("GIT_PAGER", "cat");
/// WSL 变量透传清单变量名；WSL 只把列入其中的变量传入 Linux 侧。
const WSLENV_ENV: &str = "WSLENV";
/// WSLENV 清单分隔符。
const WSLENV_SEPARATOR: &str = ":";

/// 返回本次 shell 执行应注入的环境变量。
///
/// `tty` 为真时保留调用环境的 `TERM`（交互式程序依赖真实终端能力），
/// 但仍禁用分页器：分页器挂起源于缺少交互输入，与终端能力无关。
///
/// `forwards_to_wsl` 为真时追加 `WSLENV` 清单：WSL 不继承 Windows 进程环境，
/// 未经 `WSLENV` 声明的变量在 Linux 侧不可见，仅设置宿主进程环境不会生效。
pub(crate) fn shell_env_overrides(tty: bool, forwards_to_wsl: bool) -> Vec<(String, String)> {
    let mut overrides = vec![
        (PAGER_ENV.0.to_owned(), PAGER_ENV.1.to_owned()),
        (GIT_PAGER_ENV.0.to_owned(), GIT_PAGER_ENV.1.to_owned()),
    ];
    if !tty {
        overrides.push((NO_COLOR_ENV.0.to_owned(), NO_COLOR_ENV.1.to_owned()));
        overrides.push((TERM_ENV.0.to_owned(), TERM_ENV.1.to_owned()));
    }
    if forwards_to_wsl {
        let forwarded = overrides
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>()
            .join(WSLENV_SEPARATOR);
        overrides.push((WSLENV_ENV.to_owned(), forwarded));
    }
    overrides
}

/// `pwsh` 主版本探测结果在进程内缓存：它取决于本机静态环境，
/// 每次解析 shell 都启动子进程会让判定与执行固定多付出数百毫秒。
static PWSH_MAJOR_VERSION_CACHE: OnceLock<Option<u64>> = OnceLock::new();

/// Windows PowerShell 可用性探测结果同样按进程缓存。
#[cfg(windows)]
static WINDOWS_POWERSHELL_PROBE_CACHE: OnceLock<bool> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedShell {
    pub(crate) dialect: ShellDialect,
    pub(crate) program: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellDialect {
    PosixLogin,
    #[cfg(not(windows))]
    PosixCommand,
    PowerShell,
}

impl ResolvedShell {
    pub(crate) fn default_for_current_platform() -> Self {
        #[cfg(windows)]
        {
            let program = if powershell_major_version(PWSH_PROGRAM)
                .is_some_and(|major| major >= PWSH_MAJOR_VERSION)
            {
                PWSH_PROGRAM
            } else {
                WINDOWS_POWERSHELL_PROGRAM
            };
            Self {
                dialect: ShellDialect::PowerShell,
                program: program.to_owned(),
            }
        }

        #[cfg(not(windows))]
        {
            default_unix_shell()
        }
    }
}

/// 返回 Axon 执行 `shell_command` 时采用的默认 Shell 程序。
///
/// Windows 下复用 PowerShell 7+ 的进程级探测缓存；该值表示 Axon 的实际执行 Shell，
/// 不等同于 Windows 的 `ComSpec` 默认命令解释器。
pub fn default_shell_program() -> String {
    ResolvedShell::default_for_current_platform().program
}

pub(crate) fn command_from_script(
    script: &str,
    empty_error: &'static str,
    tty: bool,
) -> AxonResult<Vec<String>> {
    command_from_script_for_shell(
        script,
        empty_error,
        ResolvedShell::default_for_current_platform(),
        tty,
    )
}

pub(crate) fn command_from_script_at_cwd(
    script: &str,
    empty_error: &'static str,
    cwd: &Path,
    tty: bool,
) -> AxonResult<Vec<String>> {
    #[cfg(windows)]
    {
        if let Some(wsl) = parse_wsl_unc_path(&cwd.to_string_lossy())? {
            let script = validated_script(script, empty_error)?;
            return wsl_command_argv(&wsl, script, tty);
        }
    }

    #[cfg(not(windows))]
    let _ = cwd;

    command_from_script(script, empty_error, tty)
}

pub(crate) fn command_from_script_for_shell(
    script: &str,
    empty_error: &'static str,
    shell: ResolvedShell,
    tty: bool,
) -> AxonResult<Vec<String>> {
    let script = validated_script(script, empty_error)?;
    shell_command_argv(&shell, script, tty)
}

pub(crate) fn shell_program_denied(command: &[String], program: &str) -> bool {
    let Some(stem) = program_stem(program) else {
        return false;
    };
    match stem.as_str() {
        "sh" | "bash" | "zsh" => !is_canonical_posix_script_command(command),
        "pwsh" | "powershell" => !is_canonical_powershell_script_command(command),
        "wsl" => !is_canonical_wsl_script_command(command),
        "fish" | "cmd" => true,
        _ => false,
    }
}

pub(crate) fn validate_shell_environment(command: &[String]) -> AxonResult<()> {
    if !is_canonical_powershell_script_command(command) {
        return Ok(());
    }
    let program = command.first().map(String::as_str).unwrap_or(PWSH_PROGRAM);
    match program_stem(program).as_deref() {
        Some("pwsh")
            if powershell_major_version(program)
                .is_some_and(|major| major >= PWSH_MAJOR_VERSION) =>
        {
            Ok(())
        }
        Some("pwsh") => Err(AxonError::tool_execution_failed(PWSH_VERSION_ERROR)),
        Some("powershell") if powershell_probe_succeeds(program) => Ok(()),
        Some("powershell") => Err(AxonError::tool_execution_failed(WINDOWS_POWERSHELL_ERROR)),
        _ => Err(AxonError::tool_execution_failed(WINDOWS_POWERSHELL_ERROR)),
    }
}

fn powershell_major_version(program: &str) -> Option<u64> {
    if program == PWSH_PROGRAM {
        return *PWSH_MAJOR_VERSION_CACHE.get_or_init(|| probe_powershell_major_version(program));
    }
    probe_powershell_major_version(program)
}

fn probe_powershell_major_version(program: &str) -> Option<u64> {
    let mut command = std::process::Command::new(program);
    configure_non_interactive_process(&mut command);
    let output = command
        .args(POWERSHELL_PIPE_ARGS_PREFIX)
        .arg(PWSH_VERSION_SCRIPT)
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .ok()
    })?
}

fn powershell_probe_succeeds(program: &str) -> bool {
    #[cfg(windows)]
    if program == WINDOWS_POWERSHELL_PROGRAM {
        return *WINDOWS_POWERSHELL_PROBE_CACHE.get_or_init(|| probe_powershell_program(program));
    }
    probe_powershell_program(program)
}

fn probe_powershell_program(program: &str) -> bool {
    let mut command = std::process::Command::new(program);
    configure_non_interactive_process(&mut command);
    command
        .args(POWERSHELL_PIPE_ARGS_PREFIX)
        .arg("$null")
        .status()
        .is_ok_and(|status| status.success())
}

fn shell_command_argv(shell: &ResolvedShell, script: &str, tty: bool) -> AxonResult<Vec<String>> {
    match shell.dialect {
        ShellDialect::PosixLogin => Ok(vec![
            shell.program.clone(),
            POSIX_LOGIN_FLAG.to_owned(),
            script.to_owned(),
        ]),
        #[cfg(not(windows))]
        ShellDialect::PosixCommand => Ok(vec![
            shell.program.clone(),
            POSIX_COMMAND_FLAG.to_owned(),
            script.to_owned(),
        ]),
        ShellDialect::PowerShell => {
            let mut command = vec![shell.program.clone()];
            command.extend(
                if tty {
                    POWERSHELL_TTY_ARGS_PREFIX
                } else {
                    POWERSHELL_PIPE_ARGS_PREFIX
                }
                .iter()
                .map(|argument| (*argument).to_owned()),
            );
            command.push(normalize_powershell_script(script)?);
            Ok(command)
        }
    }
}

/// 归一化脚本：只做非空校验，内容不做任何预检（对齐 DSH）。
///
/// 方言自洽、多行结构与危险词一律交给 shell 自身在运行期回报。
fn validated_script<'a>(script: &'a str, empty_error: &'static str) -> AxonResult<&'a str> {
    let script = script.trim();
    if script.is_empty() {
        return Err(AxonError::tool_invalid_arguments(empty_error));
    }
    Ok(script)
}

#[cfg(windows)]
#[path = "wsl_launch.rs"]
mod wsl_launch;
#[cfg(windows)]
use wsl_launch::*;

#[cfg(not(windows))]
fn default_unix_shell() -> ResolvedShell {
    std::env::var_os("SHELL")
        .and_then(|value| shell_from_path(PathBuf::from(value)))
        .unwrap_or_else(|| ResolvedShell {
            dialect: ShellDialect::PosixCommand,
            program: "/bin/sh".to_owned(),
        })
}

#[cfg(not(windows))]
fn shell_from_path(path: PathBuf) -> Option<ResolvedShell> {
    let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
    let program = path.to_string_lossy().to_string();
    match stem.as_str() {
        "bash" | "zsh" => Some(ResolvedShell {
            dialect: ShellDialect::PosixLogin,
            program,
        }),
        "sh" => Some(ResolvedShell {
            dialect: ShellDialect::PosixCommand,
            program,
        }),
        "pwsh" | "powershell" => Some(ResolvedShell {
            dialect: ShellDialect::PowerShell,
            program,
        }),
        _ => None,
    }
}

fn normalize_powershell_script(script: &str) -> AxonResult<String> {
    if script.trim_start().starts_with(POWERSHELL_UTF8_PREFIX) {
        Ok(script.to_owned())
    } else {
        Ok(format!("{POWERSHELL_UTF8_PREFIX}{script}"))
    }
}

fn is_canonical_posix_script_command(command: &[String]) -> bool {
    command.len() == 3
        && matches!(command.get(1).map(String::as_str), Some("-lc" | "-c"))
        && command
            .last()
            .is_some_and(|script| !script.trim().is_empty())
}

/// 校验 WSL 发行版名：非空且只含 ASCII 字母数字与 `-`、`_`、`.`。
///
/// 纯字符串判定，不依赖任何 Windows API；`is_canonical_wsl_script_command` 是跨平台
/// 形态校验入口，因此校验器必须与它同处跨平台模块，不能留在只在 Windows 编译的
/// `wsl_launch` 里，否则非 Windows 目标会因符号缺失报 E0425。
fn valid_wsl_distribution(distribution: &str) -> bool {
    !distribution.is_empty()
        && distribution.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

pub(crate) fn is_canonical_wsl_script_command(command: &[String]) -> bool {
    command.len() == 9
        && command
            .get(1)
            .is_some_and(|value| value == WSL_DISTRIBUTION_ARG)
        && command
            .get(2)
            .is_some_and(|value| valid_wsl_distribution(value))
        && command.get(3).is_some_and(|value| value == WSL_CWD_ARG)
        && command.get(4).is_some_and(|value| value.starts_with('/'))
        && command.get(5).is_some_and(|value| value == WSL_EXEC_ARG)
        && command
            .get(6)
            .is_some_and(|value| value == WSL_BASH_PROGRAM)
        && command
            .get(7)
            .is_some_and(|value| value == WSL_BASH_COMMAND_ARG)
        && command
            .get(8)
            .is_some_and(|script| !script.trim().is_empty())
}

pub(crate) fn is_canonical_powershell_script_command(command: &[String]) -> bool {
    if command
        .first()
        .and_then(|program| program_stem(program))
        .as_deref()
        .is_none_or(|stem| !matches!(stem, "pwsh" | "powershell"))
    {
        return false;
    }
    let Some(script) = command.last().filter(|script| !script.trim().is_empty()) else {
        return false;
    };
    let _ = script;
    // 单元素命令（如 `["pwsh"]`）会让 `1..0` 构成反向切片而 panic。
    // 该形态不可能来自生产调用点（命令行均经 `command_from_script_at_cwd` 规范化），
    // 但本函数是 crate 内公开判定入口，必须自守而非依赖调用方。
    if command.len() < 2 {
        return false;
    }
    let arguments = &command[1..command.len() - 1];
    arguments
        .iter()
        .map(String::as_str)
        .eq(POWERSHELL_PIPE_ARGS_PREFIX.iter().copied())
        || arguments
            .iter()
            .map(String::as_str)
            .eq(POWERSHELL_TTY_ARGS_PREFIX.iter().copied())
}

fn program_stem(program: &str) -> Option<String> {
    Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_ERROR: &str = "empty";

    #[test]
    #[cfg(not(windows))]
    fn posix_bash_shell_uses_login_command_flag() {
        let command = command_from_script_for_shell(
            "python3 - <<'PY'\nprint('ok')\nPY",
            EMPTY_ERROR,
            ResolvedShell {
                dialect: ShellDialect::PosixLogin,
                program: "/bin/bash".to_owned(),
            },
            false,
        )
        .unwrap();

        assert_eq!(command[0], "/bin/bash");
        assert_eq!(command[1], "-lc");
        assert!(command[2].contains("<<'PY'"));
    }

    #[test]
    #[cfg(not(windows))]
    fn posix_sh_shell_uses_plain_command_flag() {
        let command = command_from_script_for_shell(
            "echo axon",
            EMPTY_ERROR,
            ResolvedShell {
                dialect: ShellDialect::PosixCommand,
                program: "/bin/sh".to_owned(),
            },
            false,
        )
        .unwrap();

        assert_eq!(command, ["/bin/sh", "-c", "echo axon"]);
    }

    #[test]
    fn powershell_shell_prefixes_utf8() {
        let command = command_from_script_for_shell(
            "@'\nprint('ok')\n'@ | python -",
            EMPTY_ERROR,
            ResolvedShell {
                dialect: ShellDialect::PowerShell,
                program: "pwsh".to_owned(),
            },
            false,
        )
        .unwrap();

        assert_eq!(
            &command[..5],
            [
                "pwsh",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command"
            ]
        );
        assert!(command[5].starts_with(POWERSHELL_UTF8_PREFIX));
        assert!(command[5].contains("[Console]::InputEncoding=$OutputEncoding"));
        assert!(command[5].contains("$PSStyle.OutputRendering='PlainText'"));
        assert!(command[5].contains("@'\nprint('ok')\n'@ | python -"));
    }

    #[test]
    fn powershell_tty_keeps_interactive_host_enabled() {
        let command = command_from_script_for_shell(
            "$value = Read-Host; Write-Output $value",
            EMPTY_ERROR,
            ResolvedShell {
                dialect: ShellDialect::PowerShell,
                program: "pwsh".to_owned(),
            },
            true,
        )
        .unwrap();

        assert_eq!(&command[..4], ["pwsh", "-NoLogo", "-NoProfile", "-Command"]);
        assert!(!command.iter().any(|argument| argument == "-NonInteractive"));
        assert!(is_canonical_powershell_script_command(&command));
    }

    /// 防御性回归：单元素命令曾因 `1..0` 反向切片而 panic。
    ///
    /// 本函数是 crate 内公开判定入口，必须在自身而非调用方处保证不 panic：
    /// 生产路径虽经 `command_from_script_at_cwd` 规范化，但新调用点或测试夹具
    /// 传入 `["pwsh"]` 即会命中。
    #[test]
    fn single_element_powershell_command_is_rejected_without_panicking() {
        assert!(!is_canonical_powershell_script_command(
            &["pwsh".to_owned()]
        ));
        assert!(!is_canonical_powershell_script_command(&[
            "powershell".to_owned()
        ]));
        // 空切片与「程序 + 单个非脚本参数」形态同样不得 panic。
        assert!(!is_canonical_powershell_script_command(&[]));
        assert!(!is_canonical_powershell_script_command(&[
            "pwsh".to_owned(),
            "echo hi".to_owned()
        ]));
    }

    #[test]
    fn shell_wrapper_validation_allows_only_canonical_shapes() {
        assert!(!shell_program_denied(
            &[
                "/bin/bash".to_owned(),
                "-lc".to_owned(),
                "echo ok".to_owned()
            ],
            "/bin/bash",
        ));
        assert!(shell_program_denied(
            &[
                "/bin/bash".to_owned(),
                "-i".to_owned(),
                "echo no".to_owned()
            ],
            "/bin/bash",
        ));
        assert!(!shell_program_denied(
            &[
                "pwsh".to_owned(),
                "-NoLogo".to_owned(),
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-Command".to_owned(),
                "echo ok".to_owned(),
            ],
            "pwsh",
        ));
        assert!(!shell_program_denied(
            &[
                "wsl.exe".to_owned(),
                "--distribution".to_owned(),
                "Ubuntu-24.04".to_owned(),
                "--cd".to_owned(),
                "/root/work".to_owned(),
                "--exec".to_owned(),
                "/bin/bash".to_owned(),
                "-lc".to_owned(),
                "echo ok".to_owned(),
            ],
            "wsl.exe",
        ));
        assert!(shell_program_denied(
            &["wsl.exe".to_owned(), "bash".to_owned()],
            "wsl.exe",
        ));
    }

    #[test]
    #[cfg(windows)]
    fn parses_supported_wsl_unc_forms() {
        for path in [
            r"\\wsl.localhost\Ubuntu-24.04\root\work",
            r"\\wsl$\Ubuntu-24.04\root\work",
            r"\\?\UNC\wsl.localhost\Ubuntu-24.04\root\work",
        ] {
            let parsed = parse_wsl_unc_path(path).unwrap().expect("应识别 WSL UNC");
            assert_eq!(parsed.distribution, "Ubuntu-24.04");
            assert_eq!(parsed.linux_path, "/root/work");
        }
    }

    #[test]
    #[cfg(windows)]
    fn wsl_unc_cwd_uses_posix_bash_command() {
        let command = command_from_script_at_cwd(
            "python3 - <<'PY'\nprint('ok')\nPY",
            EMPTY_ERROR,
            Path::new(r"\\wsl.localhost\Ubuntu-24.04\root\work"),
            false,
        )
        .unwrap();

        assert_eq!(command[0], "wsl.exe");
        assert_eq!(command[2], "Ubuntu-24.04");
        assert_eq!(command[4], "/root/work");
        assert_eq!(command[6], "/bin/bash");
        assert_eq!(command[7], "-lc");
        assert!(command[8].contains("<<'PY'"));
    }

    /// 对齐 DSH：多行承载形式一律放行：heredoc 与 here-string 都不进入预检。
    ///
    /// 修复前这对写法互为对方的禁例：PowerShell 方言拒 Bash heredoc，POSIX 方言拒
    /// here-string，模型选哪种都被拒，shell_command 实际丧失多行脚本能力。
    #[test]
    fn multiline_scripts_are_accepted_for_every_dialect() {
        let shells = [
            ResolvedShell {
                dialect: ShellDialect::PosixLogin,
                program: "/bin/bash".to_owned(),
            },
            ResolvedShell {
                dialect: ShellDialect::PowerShell,
                program: "pwsh".to_owned(),
            },
        ];
        for shell in shells {
            for script in [
                "python - <<'PY'\nprint('ok')\nPY",
                "@'\nprint('ok')\n'@ | python -",
                "@'\nprint('ok')",
            ] {
                let command =
                    command_from_script_for_shell(script, EMPTY_ERROR, shell.clone(), false)
                        .expect("多行脚本不得被静态预检拒绝");

                assert!(
                    command
                        .last()
                        .is_some_and(|argument| argument.contains(script)),
                    "脚本必须原样进入 argv：{script}"
                );
            }
        }
    }

    /// WSL argv 必须与 `is_canonical_wsl_script_command` 的形态契约一致。
    ///
    /// 该形态由 `PosixLogin` 分支生成，本用例把「构造」与「校验」钉在一起：任一侧改动而
    /// 另一侧未跟随，都会在此立即失败，而不是等到执行入口静默拒绝。
    #[test]
    #[cfg(windows)]
    fn wsl_argv_is_canonical_for_a_unc_working_directory() {
        let cwd = WslWorkingDirectory {
            distribution: "Ubuntu-24.04".to_owned(),
            linux_path: "/root/work".to_owned(),
        };

        let command = wsl_command_argv(&cwd, "echo ok", false).unwrap();

        assert!(is_canonical_wsl_script_command(&command), "{command:?}");
        assert_eq!(
            command.last().map(String::as_str),
            Some("echo ok"),
            "脚本必须是最后一个 argv 元素"
        );
    }
}
