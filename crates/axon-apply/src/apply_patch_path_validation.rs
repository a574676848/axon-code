use std::path::{Path, PathBuf};

use super::ApplyPatchError;

pub(super) fn safe_workspace_join(root: &Path, path: &str) -> Result<PathBuf, ApplyPatchError> {
    let normalized = normalize_path_prefix(path);
    if normalized.is_empty() {
        return Err(ApplyPatchError::new("PathDiff 路径不能为空"));
    }

    let candidate = PathBuf::from(&normalized);
    let target = if candidate.is_absolute() {
        candidate
    } else {
        root.join(candidate)
    };
    reject_symlink_components(root, &target)?;
    Ok(target)
}

fn reject_symlink_components(root: &Path, target: &Path) -> Result<(), ApplyPatchError> {
    let Ok(relative) = target.strip_prefix(root) else {
        return Ok(());
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ApplyPatchError::new(format!(
                    "PathDiff 拒绝符号链接路径：{}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(ApplyPatchError::new(format!("检查工作区路径失败：{error}")));
            }
        }
    }
    Ok(())
}

fn normalize_path_prefix(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    normalized
        .strip_prefix("//?/")
        .or_else(|| normalized.strip_prefix("//./"))
        .unwrap_or(&normalized)
        .to_owned()
}
