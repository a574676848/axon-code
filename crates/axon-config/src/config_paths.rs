use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use axon_domain::{AxonError, AxonResult};

use super::{AXON_RUNTIME_DATA_DIR, AXON_RUNTIME_LOG_DIR};
use crate::{USER_AXON_DIR, USER_CONFIG_FILE};

/// 长期运行进程（如 Windows 服务模式 Daemon）显式声明的运行态根目录。
///
/// 服务以 LocalSystem 启动时 `USERPROFILE` 指向服务账户目录，而非实际用户目录；
/// 此时只有组合根注入的 `--root` 是权威来源，所有运行态路径都不得再从环境推断。
static RUNTIME_ROOT_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// 声明本进程的运行态根目录；同一进程内只允许声明同一个值。
pub fn set_runtime_root_override(root: &Path) -> AxonResult<()> {
    if let Some(existing) = RUNTIME_ROOT_OVERRIDE.get() {
        return if existing == root {
            Ok(())
        } else {
            Err(AxonError::runtime_failure("运行态根目录已声明为其他路径"))
        };
    }
    RUNTIME_ROOT_OVERRIDE
        .set(root.to_path_buf())
        .map_err(|_| AxonError::runtime_failure("无法声明运行态根目录"))
}

/// 运行态根目录环境变量：组合根用它把已解析的根目录显式传递给子进程。
pub const AXON_RUNTIME_ROOT_ENV: &str = "AXON_RUNTIME_ROOT";

fn runtime_root_override() -> Option<PathBuf> {
    if let Some(root) = RUNTIME_ROOT_OVERRIDE.get() {
        return Some(root.clone());
    }
    std::env::var_os(AXON_RUNTIME_ROOT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// 解析本进程的用户主目录。
///
/// Daemon 已声明运行态根目录时，`~/.axon` 的父目录即安装用户主目录；否则按平台从环境推断。
/// 用户级 Skills、附件归档与 MCP 配置都必须与运行态数据落在同一个用户目录下。
pub fn user_home_dir() -> AxonResult<PathBuf> {
    if let Some(root) = runtime_root_override() {
        return root
            .parent()
            .map(Path::to_path_buf)
            .ok_or(AxonError::runtime_failure("运行态根目录缺少父目录"));
    }
    user_home_from_env(
        cfg!(windows),
        std::env::var_os("USERPROFILE").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn user_home_from_env(
    is_windows: bool,
    user_profile: Option<&OsStr>,
    home: Option<&OsStr>,
) -> AxonResult<PathBuf> {
    let home_dir = if is_windows {
        user_profile.or(home)
    } else {
        home.or(user_profile)
    };
    home_dir
        .map(PathBuf::from)
        .ok_or(AxonError::runtime_failure("无法解析用户主目录"))
}

pub(crate) fn default_user_config_path() -> AxonResult<PathBuf> {
    Ok(default_user_axon_root_path()?.join(USER_CONFIG_FILE))
}

pub(crate) fn default_user_axon_root_path() -> AxonResult<PathBuf> {
    if let Some(root) = runtime_root_override() {
        return Ok(root);
    }
    default_user_axon_root_path_from_env(
        cfg!(windows),
        std::env::var_os("USERPROFILE").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

pub(crate) fn default_user_axon_root_path_from_env(
    is_windows: bool,
    user_profile: Option<&OsStr>,
    home: Option<&OsStr>,
) -> AxonResult<PathBuf> {
    Ok(user_home_from_env(is_windows, user_profile, home)?.join(USER_AXON_DIR))
}

/// 解析运行态 Axon 根目录。
pub fn runtime_axon_root_path(config_path: &Path) -> AxonResult<PathBuf> {
    let user_root = default_user_axon_root_path()?;
    if is_default_or_project_config_path(config_path, &user_root) {
        return Ok(user_root);
    }
    config_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or(AxonError::runtime_failure("无法解析配置所在目录"))
}

/// 解析运行态 Axon 日志目录。
pub fn runtime_axon_log_path(config_path: &Path) -> AxonResult<PathBuf> {
    Ok(runtime_axon_root_path(config_path)?
        .join(AXON_RUNTIME_DATA_DIR)
        .join(AXON_RUNTIME_LOG_DIR))
}

fn is_default_or_project_config_path(config_path: &Path, user_root: &Path) -> bool {
    if paths_equal(config_path, &user_root.join(USER_CONFIG_FILE)) {
        return true;
    }
    config_path
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .is_some_and(|name| name == USER_AXON_DIR)
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left.components().collect::<PathBuf>() == right.components().collect::<PathBuf>()
}
