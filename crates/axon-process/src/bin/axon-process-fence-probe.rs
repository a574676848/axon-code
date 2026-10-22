//! 资源围栏测试探针：经 [`apply_process_resource_limits`] 自我套用围栏，报告内核
//! 实际接受的限额，并执行传入的自检命令。
//!
//! 本程序只在 `cargo test -p axon-process` 期间由 `resource_fence_tests` 启动。
//! 它让「内核是否真的接受了限额」变成可断言的事实：探针在**设置之后**读回
//! `/proc/self/limits`，因此读到的是内核记录值，而非测试的期望值。
//!
//! 另有一个模式 `--spawn-fenced-child-and-exit`：派生受围栏的孙进程后立即退出，
//! 用于把「宿主死亡 → 子进程被回收」也变成可断言的行为。

use std::{
    ffi::OsStr,
    io,
    process::{Command, Stdio},
};

use axon_process::resource_fence::{
    PROCESS_RLIMIT_AS_ENV, PROCESS_RLIMIT_FSIZE_ENV, PROCESS_RLIMIT_NOFILE_ENV,
    PROCESS_RLIMIT_NPROC_ENV, ProcessResourceLimits, apply_process_resource_limits,
    configure_process_resource_limits, fence_spawned_child,
};

/// `/proc/self/limits` 的标签与对应输出字段名。
const LIMIT_FIELDS: &[(&str, &str)] = &[
    ("Max file size", "fsize"),
    ("Max processes", "nproc"),
    ("Max open files", "nofile"),
    ("Max address space", "as"),
];

/// 探针自检命令的退出码输出前缀。
const ACTION_EXIT_PREFIX: &str = "action-exit=";

/// 执行自检命令的 shell。
///
/// 必须是 bash：自检命令用 `{fd}>` 动态分配描述符，dash 不支持该写法。
const ACTION_SHELL: &str = "/bin/bash";

/// 自检命令无法执行的退出码。
const ACTION_FAILED_EXIT_CODE: i32 = 1;

/// 探针模式：派生受围栏的孙进程后**立即退出**（不 wait、不终止它）。
///
/// 探针退出即模拟宿主死亡，孙进程 PID 经 stdout 交给测试进程做存活检查。
const SPAWN_CHILD_MODE: &str = "--spawn-fenced-child-and-exit";

/// 孙进程 PID 的输出前缀。
const CHILD_PID_PREFIX: &str = "child-pid=";

/// 孙进程的存活时长（秒）；必须远长于测试的等待窗口，否则「已消失」不再是有效判据。
const CHILD_LIFETIME_SECONDS: &str = "300";

/// 孙进程程序；`sleep` 在所有 Unix 上都是独立可执行文件，便于被信号终止。
const CHILD_PROGRAM: &str = "sleep";

fn main() -> io::Result<()> {
    if std::env::args_os().nth(1).as_deref() == Some(OsStr::new(SPAWN_CHILD_MODE)) {
        return spawn_fenced_child_then_exit();
    }
    apply_process_resource_limits(limits_from_environment())?;
    report_limits();
    if let Some(action) = std::env::args_os().nth(1) {
        report_action(&action);
    }
    Ok(())
}

/// 派生受围栏的孙进程并立即返回（进程随即退出）。
///
/// 围栏句柄被**故意泄漏**（`mem::forget`）而不是随函数返回而释放：本模式要验证的是
/// 「宿主进程消失 → 子进程被回收」，若在函数返回时释放句柄，Windows 侧就会因为句柄
/// 提前关闭而被回收：那样测到的是另一条路径，不再是宿主死亡的语义。
fn spawn_fenced_child_then_exit() -> io::Result<()> {
    let mut command = Command::new(CHILD_PROGRAM);
    command
        .arg(CHILD_LIFETIME_SECONDS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_process_resource_limits(&mut command, ProcessResourceLimits::off());
    let child = command.spawn()?;
    let fence = fence_spawned_child(&child, ProcessResourceLimits::off())?;
    std::mem::forget(fence);
    println!("{CHILD_PID_PREFIX}{}", child.id());
    Ok(())
}

/// 从环境变量读取待施加的限额；**未设置**的维度保持 `None`（不限制）。
///
/// 未设置的维度必须真的是 `None`：若把「未设置」当成部署默认值，`off` 档位就会
/// 被探针悄悄重新设限，测试也就无法证明「关闭围栏后确实没有限制」。
fn limits_from_environment() -> ProcessResourceLimits {
    ProcessResourceLimits {
        max_file_bytes: limit_from_environment(PROCESS_RLIMIT_FSIZE_ENV),
        max_processes: limit_from_environment(PROCESS_RLIMIT_NPROC_ENV),
        max_open_files: limit_from_environment(PROCESS_RLIMIT_NOFILE_ENV),
        max_address_space_bytes: limit_from_environment(PROCESS_RLIMIT_AS_ENV),
    }
}

/// 读取单个维度的限额；未设置或非法取值返回 `None`。
fn limit_from_environment(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
}

/// 输出内核记录的四类限额，格式为 `字段名 <软限制>`；`unlimited` 原样输出。
fn report_limits() {
    let Ok(contents) = std::fs::read_to_string("/proc/self/limits") else {
        return;
    };
    for (label, field) in LIMIT_FIELDS {
        let Some(soft_value) = limits_soft_value(&contents, label) else {
            continue;
        };
        println!("{field} {soft_value}");
    }
}

/// 从 `/proc/self/limits` 文本中取出指定标签的软限制原文。
///
/// 每行形如 `Max open files            1024    1048576    files`：
/// 标签占 1..=2 个词，之后是软限制、硬限制与单位，因此软限制是倒数第三个词。
fn limits_soft_value(contents: &str, label: &str) -> Option<String> {
    let line = contents.lines().find(|line| line.starts_with(label))?;
    let fields = line.split_whitespace().collect::<Vec<_>>();
    fields
        .len()
        .checked_sub(3)
        .and_then(|index| fields.get(index))
        .map(|value| (*value).to_owned())
}

/// 执行自检命令并输出其退出码；命令不带围栏，观察的是它继承到的限额。
fn report_action(action: &OsStr) {
    let exit_code = Command::new(ACTION_SHELL)
        .arg("-c")
        .arg(action)
        .stdin(std::process::Stdio::null())
        .status()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(ACTION_FAILED_EXIT_CODE);
    println!("{ACTION_EXIT_PREFIX}{exit_code}");
}
