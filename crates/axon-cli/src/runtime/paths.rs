//! CLI 运行态路径解析。

use std::path::{Path, PathBuf};

#[cfg(windows)]
use sha2::{Digest as _, Sha256};

pub(crate) const DEFAULT_DAEMON_ENDPOINT: &str = "axon-daemon";

/// Windows 非默认根目录端点指纹长度（十六进制字符数）。
#[cfg(windows)]
const WINDOWS_ROOT_FINGERPRINT_HEX_LEN: usize = 12;

/// 解析用户级 Axon 运行态根目录。
pub(crate) fn runtime_axon_root_path(config_path: &Path) -> Result<PathBuf, String> {
    axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())
}

/// 解析用户级 Axon 运行态根目录，并返回静态错误消息。
pub(crate) fn runtime_axon_root_path_static(config_path: &Path) -> Result<PathBuf, &'static str> {
    axon_config::runtime_axon_root_path(config_path).map_err(|_| "无法解析用户级 Axon 目录")
}

/// 解析用户级 Axon 运行数据目录，并返回静态错误消息。
pub(crate) fn runtime_axon_data_path_static(config_path: &Path) -> Result<PathBuf, &'static str> {
    Ok(runtime_axon_root_path_static(config_path)?.join(axon_config::AXON_RUNTIME_DATA_DIR))
}

pub(crate) fn daemon_endpoint_for_root(root: &Path) -> String {
    #[cfg(windows)]
    {
        windows_daemon_endpoint(root)
    }
    #[cfg(not(windows))]
    {
        root.join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(DEFAULT_DAEMON_ENDPOINT)
            .to_string_lossy()
            .into_owned()
    }
}

/// Windows 端点解析：默认用户根沿用稳定管道名；其他运行根使用根指纹隔离端点，
/// 避免临时配置或其他运行根误连到默认 Daemon。
#[cfg(windows)]
fn windows_daemon_endpoint(root: &Path) -> String {
    if is_default_user_axon_root(root) {
        return DEFAULT_DAEMON_ENDPOINT.to_owned();
    }
    format!(
        "{DEFAULT_DAEMON_ENDPOINT}-{}",
        windows_root_fingerprint(root)
    )
}

#[cfg(windows)]
fn is_default_user_axon_root(root: &Path) -> bool {
    axon_config::ConfigLoader::default_user_axon_root_path()
        .is_ok_and(|default_root| windows_root_key(&default_root) == windows_root_key(root))
}

/// Windows 路径键：统一分隔符与大小写，保证服务端与客户端计算同一端点。
#[cfg(windows)]
fn windows_root_key(root: &Path) -> String {
    root.to_string_lossy().replace('/', "\\").to_lowercase()
}

#[cfg(windows)]
fn windows_root_fingerprint(root: &Path) -> String {
    let digest = hex::encode(Sha256::digest(windows_root_key(root).as_bytes()));
    digest[..WINDOWS_ROOT_FINGERPRINT_HEX_LEN].to_owned()
}

pub(crate) fn resolve_daemon_endpoint(
    endpoint: &str,
    config_path: Option<&Path>,
) -> Result<String, String> {
    if endpoint != DEFAULT_DAEMON_ENDPOINT {
        return Ok(endpoint.to_owned());
    }
    let path = config_path
        .map(Path::to_path_buf)
        .or_else(|| axon_config::ConfigLoader::default_user_config_path().ok())
        .ok_or_else(|| "无法解析用户配置路径".to_owned())?;
    let root = runtime_axon_root_path(&path)?;
    Ok(daemon_endpoint_for_root(&root))
}

/// 解析运行态 Axon 日志目录，并返回静态错误消息。
pub(crate) fn runtime_axon_log_path_static(config_path: &Path) -> Result<PathBuf, &'static str> {
    axon_config::runtime_axon_log_path(config_path).map_err(|_| "无法解析 Axon 日志目录")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_default_root_uses_stable_daemon_endpoint() {
        let root = axon_config::ConfigLoader::default_user_axon_root_path()
            .expect("测试环境应能解析默认用户根目录");

        assert_eq!(daemon_endpoint_for_root(&root), DEFAULT_DAEMON_ENDPOINT);
    }

    #[cfg(windows)]
    #[test]
    fn windows_non_default_roots_get_isolated_endpoints() {
        let user_root = axon_config::ConfigLoader::default_user_axon_root_path()
            .expect("测试环境应能解析默认用户根目录");
        let lower_root = user_root.join("..").join("axon-alt-root");
        let upper_root = user_root.join("..").join("AXON-ALT-ROOT");

        let endpoint = daemon_endpoint_for_root(&lower_root);
        assert_ne!(endpoint, DEFAULT_DAEMON_ENDPOINT);
        assert!(endpoint.starts_with(&format!("{DEFAULT_DAEMON_ENDPOINT}-")));
        assert_eq!(
            endpoint.len(),
            DEFAULT_DAEMON_ENDPOINT.len() + 1 + WINDOWS_ROOT_FINGERPRINT_HEX_LEN
        );
        assert_eq!(endpoint, daemon_endpoint_for_root(&upper_root));
    }
}
