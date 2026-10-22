//! PTY 会话的 `prlimit` 包装解析。
//!
//! 从 `resource_fence.rs` 拆出（INV-7 单文件 700 行铁律）。这里只有**安全**代码；
//! 平台围栏执行与 `unsafe` 边界仍留在父模块。子模块可直接使用父模块的私有项
//! （Rust 可见性：后代可见祖先的私有项），因此限额类型无需重复定义。

use super::ProcessResourceLimits;

/// `prlimit` 的候选绝对路径；命中即用，避免依赖 PATH 被篡改。
const PRLIMIT_CANDIDATE_PATHS: &[&str] = &["/usr/bin/prlimit", "/bin/prlimit"];

/// `prlimit` 的文件名，用于 PATH 回退查找。
const PRLIMIT_PROGRAM: &str = "prlimit";

/// 解析可用的 PTY 包装程序绝对路径。
///
/// PATH 回退查找在两个平台上都执行，以保证查找代码始终被编译与检查；
/// Windows 侧无条件返回 `None`：`portable_pty` 的 ConPTY 启动路径不返回子进程句柄，
/// 也不接受外部 Job，因此该平台无法为 PTY 会话附加围栏。
pub(crate) fn resolved_pty_wrapper() -> Option<String> {
    use std::sync::LazyLock;

    static WRAPPER: LazyLock<Option<String>> = LazyLock::new(|| {
        let found = find_program(PRLIMIT_PROGRAM, PRLIMIT_CANDIDATE_PATHS, &path_entries());
        if cfg!(unix) { found } else { None }
    });
    WRAPPER.clone()
}

/// 组装 PTY 的 `prlimit` 限额参数；未设限的维度不产生参数。
pub(crate) fn pty_limit_arguments(limits: ProcessResourceLimits) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut push = |flag: &str, value: Option<u64>| {
        if let Some(value) = value {
            arguments.push(format!("--{flag}={value}"));
        }
    };
    push("fsize", limits.max_file_bytes);
    push("nproc", limits.max_processes);
    push("nofile", limits.max_open_files);
    push("as", limits.max_address_space_bytes);
    arguments
}

/// 在候选绝对路径与 PATH 目录中查找可执行程序。
pub(crate) fn find_program(
    program: &str,
    candidates: &[&str],
    path_entries: &[String],
) -> Option<String> {
    for candidate in candidates {
        if std::path::Path::new(candidate).is_file() {
            return Some((*candidate).to_owned());
        }
    }
    path_entries
        .iter()
        .map(|entry| std::path::Path::new(entry).join(program))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

/// 读取 PATH 中的目录项。
///
/// PATH 回退查找是 Unix 专有行为（PTY 包装依赖 `prlimit`）。
fn path_entries() -> Vec<String> {
    std::env::var_os("PATH")
        .map(|value| {
            std::env::split_paths(&value)
                .map(|entry| entry.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}
