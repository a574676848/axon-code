//! 受管 release 的运行引用登记。

use std::{
    env, fs,
    path::{Path, PathBuf},
};

const VERSIONS_DIR: &str = "versions";
const STATE_DIR: &str = "state";
const LEASES_DIR: &str = "runtime-release-leases";

/// 当前进程对其启动 release 的短生命周期引用。
pub(crate) struct RuntimeReleaseLease {
    path: PathBuf,
}

impl Drop for RuntimeReleaseLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        super::upgrade_apply::cleanup_releases_after_startup();
    }
}

/// 仅在受管 release 布局内登记当前进程；旧布局和登记失败均不影响命令执行。
pub(crate) fn acquire_runtime_release_lease() -> Option<RuntimeReleaseLease> {
    register_runtime_release(&env::current_exe().ok()?, std::process::id())
}

fn register_runtime_release(executable: &Path, pid: u32) -> Option<RuntimeReleaseLease> {
    let executable = fs::canonicalize(executable).ok()?;
    let release = executable.parent()?;
    let releases = release.parent()?;
    if releases.file_name().and_then(|name| name.to_str()) != Some(VERSIONS_DIR) {
        return None;
    }
    let root = releases.parent()?;
    let lease_dir = root.join(STATE_DIR).join(LEASES_DIR);
    fs::create_dir_all(&lease_dir).ok()?;
    let path = lease_dir.join(format!("{pid}.lease"));
    fs::write(&path, release.to_string_lossy().as_bytes()).ok()?;
    Some(RuntimeReleaseLease { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_release_lease_records_release_and_is_removed_on_drop() {
        let root = axon_test_support::test_temp_dir();
        let release = root.path().join(VERSIONS_DIR).join("1.2.3-test");
        fs::create_dir_all(&release).expect("release 目录应可创建");
        let executable = release.join("axon");
        fs::write(&executable, b"test").expect("测试二进制应可写入");

        let lease = register_runtime_release(&executable, 42).expect("应登记受管 release");
        assert_eq!(
            fs::read_to_string(&lease.path).ok(),
            fs::canonicalize(&release)
                .ok()
                .map(|path| path.display().to_string())
        );
        let path = lease.path.clone();
        drop(lease);
        assert!(!path.exists());
    }
}
