//! Code symbol memory support helpers.

use std::{fs, path::Path};

use axon_domain::{AxonError, AxonResult};

pub(super) fn validate_project_root(project_root: &Path) -> AxonResult<()> {
    let metadata = fs::metadata(project_root)
        .map_err(|_| AxonError::runtime_failure("代码符号项目根目录不可读"))?;
    if !metadata.is_dir() {
        return Err(AxonError::runtime_failure("代码符号项目根目录无效"));
    }
    Ok(())
}
