//! Shell 环境净化通道的回归测试。
//!
//! 分两层覆盖：
//!
//! 1. **策略层**（纯函数）：非 TTY / TTY / WSL 三种情形下应注入哪些变量。
//! 2. **机制层**（真实进程）：`ProcessBackend` 是否真的把给定变量交给子进程。
//!
//! 机制层刻意**不**复用 `shell_env_overrides` 的取值，而是注入一个父进程不会设置的
//! 哨兵变量。宿主环境可能已带有 `PAGER=cat`（例如 Axon 自身就是被某个已净化的
//! 终端启动的），若断言注入值本身，未注入时测试仍会通过：那只是继承来的假象。

use std::{fs, path::PathBuf, sync::Arc, thread, time::Duration, time::Instant};

use super::{
    ShellCommandToolRuntime, output_buffer::SharedProcessOutput, process_backend::ProcessBackend,
};
use crate::builtin_shell_script::shell_env_overrides;

/// 后端探测用例等待进程退出的期限。
const BACKEND_EXIT_DEADLINE: Duration = Duration::from_secs(10);
/// 后端探测用例的轮询间隔。
const BACKEND_POLL_INTERVAL: Duration = Duration::from_millis(20);
/// 分页器变量名。
const PAGER_KEY: &str = "PAGER";
/// git 分页器变量名。
const GIT_PAGER_KEY: &str = "GIT_PAGER";
/// 色彩禁用变量名。
const NO_COLOR_KEY: &str = "NO_COLOR";
/// 终端类型变量名。
const TERM_KEY: &str = "TERM";
/// 分页器替代值。
const PAGER_VALUE: &str = "cat";
/// WSL 变量透传声明变量名。
const WSLENV_KEY: &str = "WSLENV";
/// 透传声明的条目分隔符。
const WSLENV_ENTRY_SEPARATOR: &str = ":";
/// 非 TTY 场景应注入的变量数量：两个分页器 + 色彩 + 终端类型。
const NON_TTY_VARIABLE_COUNT: usize = 4;
/// TTY 场景应注入的变量数量：仅两个分页器。
const TTY_VARIABLE_COUNT: usize = 2;
/// 机制探测用的哨兵变量名；父进程不会设置它，可见性只能来自注入。
const SENTINEL_KEY: &str = "AXON_SHELL_ENV_PROBE";
/// 机制探测用的哨兵取值。
const SENTINEL_VALUE: &str = "injected-by-axon";
/// 探测命令的输出前缀。
const PROBE_PREFIX: &str = "env-probe=[";

#[test]
fn non_tty_execution_disables_pager_color_and_terminal_capabilities() {
    let overrides = shell_env_overrides(false, false);

    assert_eq!(overrides.len(), NON_TTY_VARIABLE_COUNT, "{overrides:?}");
    assert_eq!(value_of(&overrides, PAGER_KEY), Some(PAGER_VALUE));
    assert_eq!(value_of(&overrides, GIT_PAGER_KEY), Some(PAGER_VALUE));
    assert_eq!(value_of(&overrides, NO_COLOR_KEY), Some("1"));
    assert_eq!(value_of(&overrides, TERM_KEY), Some("dumb"));
}

#[test]
fn tty_execution_keeps_terminal_capabilities_but_disables_pagers() {
    let overrides = shell_env_overrides(true, false);

    assert_eq!(overrides.len(), TTY_VARIABLE_COUNT, "{overrides:?}");
    assert_eq!(value_of(&overrides, PAGER_KEY), Some(PAGER_VALUE));
    assert_eq!(value_of(&overrides, GIT_PAGER_KEY), Some(PAGER_VALUE));
    assert_eq!(
        value_of(&overrides, TERM_KEY),
        None,
        "交互式程序依赖真实终端能力，TTY 场景不得把 TERM 降级为 dumb"
    );
    assert_eq!(value_of(&overrides, NO_COLOR_KEY), None);
}

#[test]
fn wsl_execution_declares_every_injected_variable_for_forwarding() {
    let overrides = shell_env_overrides(false, true);
    let forwarded = value_of(&overrides, WSLENV_KEY).expect("WSL 执行必须声明 WSLENV");
    let declared = forwarded.split(WSLENV_ENTRY_SEPARATOR).collect::<Vec<_>>();
    let injected = overrides
        .iter()
        .map(|(key, _)| key.as_str())
        .filter(|key| *key != WSLENV_KEY)
        .collect::<Vec<_>>();

    assert_eq!(
        declared, injected,
        "WSLENV 清单必须与实际注入的变量完全一致，否则 Linux 侧会缺少变量"
    );
}

#[test]
fn non_wsl_execution_omits_forwarding_declaration() {
    assert_eq!(
        value_of(&shell_env_overrides(false, false), WSLENV_KEY),
        None
    );
    assert_eq!(
        value_of(&shell_env_overrides(true, false), WSLENV_KEY),
        None
    );
}

#[test]
fn pipe_backend_applies_injected_environment() {
    let preview = backend_env_probe(false, &sentinel_overrides());

    assert!(
        preview.contains(&probe_output()),
        "pipe 后端未把注入变量交给子进程，实际输出为 {preview:?}"
    );
}

#[test]
fn pty_backend_applies_injected_environment() {
    let preview = backend_env_probe(true, &sentinel_overrides());

    assert!(
        preview.contains(&probe_output()),
        "PTY 后端未把注入变量交给子进程，实际输出为 {preview:?}"
    );
}

#[test]
fn backend_omits_variables_that_were_not_injected() {
    // 反向对照：未列入注入清单的哨兵不得出现在子进程环境中。
    // 若宿主进程恰好带有该哨兵，本用例会失败，从而暴露上面两个用例的假阳性：
    // 那种情况下「探测到取值」只说明环境被继承，而不是注入生效。
    let preview = backend_env_probe(false, &[]);

    assert!(
        preview.contains(PROBE_PREFIX),
        "探测命令本身应正常执行，实际输出为 {preview:?}"
    );
    assert!(
        !preview.contains(&probe_output()),
        "哨兵未注入却出现在子进程环境中：宿主环境已带有该变量，机制用例失去判别力"
    );
}

fn sentinel_overrides() -> Vec<(String, String)> {
    vec![(SENTINEL_KEY.to_owned(), SENTINEL_VALUE.to_owned())]
}

fn probe_output() -> String {
    format!("{PROBE_PREFIX}{SENTINEL_VALUE}]")
}

/// 以给定注入清单启动一次真实进程，返回其 stdout 预览。
fn backend_env_probe(tty: bool, overrides: &[(String, String)]) -> String {
    let (artifact, path) = temp_artifact("env");
    let output = Arc::new(SharedProcessOutput::new(
        artifact,
        crate::runtime_limits::SHELL_ARTIFACT_MAX_BYTES,
    ));
    let command = sentinel_probe_command(tty);
    let cwd = std::env::current_dir().expect("测试工作目录应可解析");
    let mut backend = ProcessBackend::spawn(&command, &cwd, tty, Arc::clone(&output), overrides)
        .expect("后端应启动成功");
    wait_for_exit(&mut backend);
    let preview = output.take_receipt(None).stdout.preview;
    let _ = fs::remove_file(&path);
    preview
}

fn wait_for_exit(backend: &mut ProcessBackend) {
    let started = Instant::now();
    while started.elapsed() < BACKEND_EXIT_DEADLINE {
        if backend
            .try_wait()
            .expect("轮询子进程状态不应失败")
            .is_some()
        {
            return;
        }
        thread::sleep(BACKEND_POLL_INTERVAL);
    }
}

fn value_of<'a>(overrides: &'a [(String, String)], key: &str) -> Option<&'a str> {
    overrides
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

/// 输出 `env-probe=[<哨兵取值>]`；用括号包裹便于把命令回显与真实取值区分开。
#[cfg(windows)]
fn sentinel_probe_command(tty: bool) -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(
        &format!("Write-Output \"{PROBE_PREFIX}$env:{SENTINEL_KEY}]\""),
        tty,
    )
    .expect("哨兵探测命令应规范化成功")
}

#[cfg(unix)]
fn sentinel_probe_command(tty: bool) -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(
        &format!("printf '{PROBE_PREFIX}%s]\\n' \"${SENTINEL_KEY}\""),
        tty,
    )
    .expect("哨兵探测命令应规范化成功")
}

fn temp_artifact(tag: &str) -> (fs::File, PathBuf) {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-shell-{tag}-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let file = fs::File::create(&path).expect("临时文件应创建成功");
    (file, path)
}
