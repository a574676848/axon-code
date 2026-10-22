//! 文件遍历错误映射。

use axon_domain::AxonError;

pub(crate) const WALK_ROOT_UNREADABLE: &str = "遍历根目录不可读";
pub(crate) const WALK_ROOT_NOT_DIRECTORY: &str = "遍历根目录必须是目录";

pub(crate) fn root_unreadable() -> AxonError {
    AxonError::runtime_failure(WALK_ROOT_UNREADABLE)
}

pub(crate) fn root_not_directory() -> AxonError {
    AxonError::runtime_failure(WALK_ROOT_NOT_DIRECTORY)
}
