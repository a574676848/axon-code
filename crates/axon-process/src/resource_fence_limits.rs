//! 子进程资源限额模型与配置解析。
//!
//! 从 `resource_fence.rs` 拆出（INV-7 单文件 700 行铁律）：本模块只含**安全**代码，
//! 平台围栏执行与 `unsafe` 边界仍留在父模块。
//!
//! 子模块可直接使用父模块的私有常量（Rust 可见性：后代可见祖先私有项），
//! 因此限额默认值与环境变量名仍集中定义在父模块，避免两处漂移。

use super::{
    DEFAULT_RLIMIT_AS_BYTES, DEFAULT_RLIMIT_FSIZE_BYTES, DEFAULT_RLIMIT_NOFILE,
    DEFAULT_RLIMIT_NPROC, PROCESS_RLIMIT_AS_ENV, PROCESS_RLIMIT_FSIZE_ENV,
    PROCESS_RLIMIT_NOFILE_ENV, PROCESS_RLIMIT_NPROC_ENV, PTY_RLIMIT_FSIZE_BYTES, PTY_RLIMIT_NOFILE,
    PTY_RLIMIT_NPROC, RESOURCE_LIMIT_DISABLED_VALUE,
};

/// 子进程资源围栏配置；`None` 表示该维度不限制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcessResourceLimits {
    /// 文件大小上限；`None` 表示不限制。
    pub max_file_bytes: Option<u64>,
    /// 进程数上限；`None` 表示不限制。
    pub max_processes: Option<u64>,
    /// 文件描述符上限；`None` 表示不限制。
    pub max_open_files: Option<u64>,
    /// 地址空间上限；`None` 表示不限制。
    pub max_address_space_bytes: Option<u64>,
}

impl ProcessResourceLimits {
    /// 部署默认限额：宽到不影响正常构建，紧到能阻断失控写入与进程爆炸。
    #[must_use]
    pub const fn deployment_default() -> Self {
        Self {
            max_file_bytes: Some(DEFAULT_RLIMIT_FSIZE_BYTES),
            max_processes: Some(DEFAULT_RLIMIT_NPROC),
            max_open_files: Some(DEFAULT_RLIMIT_NOFILE),
            max_address_space_bytes: DEFAULT_RLIMIT_AS_BYTES,
        }
    }

    /// PTY 交互式会话限额；比一次性命令更宽，理由见各 `PTY_RLIMIT_*` 常量。
    #[must_use]
    pub const fn interactive_default() -> Self {
        Self {
            max_file_bytes: Some(PTY_RLIMIT_FSIZE_BYTES),
            max_processes: Some(PTY_RLIMIT_NPROC),
            max_open_files: Some(PTY_RLIMIT_NOFILE),
            max_address_space_bytes: DEFAULT_RLIMIT_AS_BYTES,
        }
    }

    /// 完全不限制；用于排查围栏本身导致的故障。
    #[must_use]
    pub const fn off() -> Self {
        Self {
            max_file_bytes: None,
            max_processes: None,
            max_open_files: None,
            max_address_space_bytes: None,
        }
    }

    /// 是否完全未设限。
    #[must_use]
    pub const fn is_off(self) -> bool {
        self.max_file_bytes.is_none()
            && self.max_processes.is_none()
            && self.max_open_files.is_none()
            && self.max_address_space_bytes.is_none()
    }

    /// 在部署默认值之上叠加环境变量覆盖。
    ///
    /// 每个维度独立覆盖：环境变量优先于部署默认值，`off` 或 `0` 表示该维度不限制。
    /// 每次调用都重新读取环境，因此排查围栏故障时放下围栏无需重新编译。
    #[must_use]
    pub fn resolved_default() -> Self {
        let base = Self::deployment_default();
        Self {
            max_file_bytes: resolve_environment_limit(
                PROCESS_RLIMIT_FSIZE_ENV,
                base.max_file_bytes,
            ),
            max_processes: resolve_environment_limit(PROCESS_RLIMIT_NPROC_ENV, base.max_processes),
            max_open_files: resolve_environment_limit(
                PROCESS_RLIMIT_NOFILE_ENV,
                base.max_open_files,
            ),
            max_address_space_bytes: resolve_environment_limit(
                PROCESS_RLIMIT_AS_ENV,
                base.max_address_space_bytes,
            ),
        }
    }
}

/// 在给定取值上应用覆盖规则：未设置时回落默认，非法取值同样回落默认。
///
/// 取值由调用方读出后传入，因此本函数是**纯函数**：测试可直接以入参驱动，
/// 无需改动真实进程环境（`set_var`/`remove_var` 会污染宿主，且在被测进程内并发不安全）。
pub(super) fn apply_environment_override(
    value: Option<&str>,
    fallback: Option<u64>,
) -> Option<u64> {
    match value {
        Some(value) => parse_environment_limit(value).unwrap_or(fallback),
        None => fallback,
    }
}

/// 解析单个环境变量覆盖项；非法取值回落到部署默认值。
fn resolve_environment_limit(name: &str, fallback: Option<u64>) -> Option<u64> {
    apply_environment_override(std::env::var(name).ok().as_deref(), fallback)
}

/// 解析环境变量取值：`off` 或 `0` 表示不限制，非法取值返回 `None` 交由调用方回落。
pub(super) fn parse_environment_limit(value: &str) -> Option<Option<u64>> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case(RESOURCE_LIMIT_DISABLED_VALUE) || trimmed == "0" {
        return Some(None);
    }
    trimmed.parse::<u64>().ok().map(Some)
}
