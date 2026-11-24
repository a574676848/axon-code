//! LSP 文档 URI 与路径辅助。

use std::path::{Path, PathBuf};

use axon_domain::{AxonError, AxonResult};

pub(crate) fn path_to_file_uri(path: &Path) -> AxonResult<String> {
    url::Url::from_file_path(normalize_file_path(path))
        .map(String::from)
        .map_err(|()| AxonError::tool_invalid_definition("无法转换 LSP 文件 URI"))
}

pub(crate) fn file_path_from_uri(uri: &str) -> AxonResult<PathBuf> {
    url::Url::parse(uri)
        .ok()
        .and_then(|value| value.to_file_path().ok())
        .ok_or_else(|| AxonError::tool_invalid_definition("LSP document URI 不是本机文件"))
}

pub(crate) fn canonical_file_path_from_uri(uri: &str) -> AxonResult<PathBuf> {
    let path = file_path_from_uri(uri)?;
    Ok(path.canonicalize().unwrap_or(path))
}

#[cfg(windows)]
fn normalize_file_path(path: &Path) -> PathBuf {
    const VERBATIM_UNC_PREFIX: &str = r"\\?\UNC\";
    const VERBATIM_PREFIX: &str = r"\\?\";
    const UNC_PREFIX: &str = r"\\";

    let value = path.to_string_lossy();
    if let Some(remainder) = value.strip_prefix(VERBATIM_UNC_PREFIX) {
        return PathBuf::from(format!("{UNC_PREFIX}{remainder}"));
    }
    PathBuf::from(value.strip_prefix(VERBATIM_PREFIX).unwrap_or(&value))
}

#[cfg(not(windows))]
fn normalize_file_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}
