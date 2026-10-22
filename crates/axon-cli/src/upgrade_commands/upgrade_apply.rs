//! `axon upgrade --apply` 的不可变 release 发布实现。

use std::{
    collections::BTreeSet,
    env,
    fs::{self, File},
    path::{Path, PathBuf},
};

use axon_fs_walk::{EntryKind, Walk, WalkAction, WalkOpts};
use axon_process::non_interactive_command;
use serde::{Deserialize, Serialize};

use super::UpgradeApplyReport;

const VERSIONS_DIR: &str = "versions";
const CURRENT_MANIFEST: &str = "current.json";
const PREVIOUS_MANIFEST: &str = "previous.json";
const INSTALL_LOCK: &str = "install.lock";
const STATE_DIR: &str = "state";
const RUNTIME_RELEASE_LEASES_DIR: &str = "runtime-release-leases";

#[derive(Debug, Deserialize, Serialize)]
struct CurrentManifest {
    #[serde(rename = "versionId")]
    version_id: String,
}

pub(super) fn apply_upgrade(
    archive_path: &str,
    _expected_sha256: &str,
    version: &str,
    target: &str,
) -> Result<UpgradeApplyReport, String> {
    let archive = Path::new(archive_path);
    if !archive.is_file() {
        return Err(format!("下载的 archive 不存在: {}", archive.display()));
    }
    let install_root = managed_install_root()?;
    let _lock = acquire_install_lock(&install_root)?;
    let release_name = release_name(version, target)?;
    let release_dir = install_root.join(VERSIONS_DIR).join(&release_name);

    if !release_dir.exists() {
        publish_release(archive, &install_root, &release_name, &release_dir)?;
    }
    let release_bin = release_binary(&release_dir)?;
    if !smoke_test(&release_bin) {
        return Err(format!(
            "release 无法通过 axon --version 验证: {}",
            release_bin.display()
        ));
    }

    let previous = active_release(&install_root).ok();
    activate_release(&install_root, &release_dir, previous.as_deref())?;
    restart_daemon_after_upgrade(&install_root)?;
    warn_if_release_gc_fails(&install_root);
    Ok(UpgradeApplyReport {
        backup_path: previous
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default(),
        install_path: install_root
            .join(CURRENT_MANIFEST)
            .to_string_lossy()
            .to_string(),
        rolled_back: false,
    })
}

pub(super) fn rollback_upgrade() -> Result<UpgradeApplyReport, String> {
    let install_root = managed_install_root()?;
    let _lock = acquire_install_lock(&install_root)?;
    let previous = active_manifest_release(&install_root, PREVIOUS_MANIFEST)?;
    let current = active_release(&install_root)?;
    activate_release(&install_root, &previous, Some(&current))?;
    warn_if_release_gc_fails(&install_root);
    Ok(UpgradeApplyReport {
        backup_path: current.to_string_lossy().to_string(),
        install_path: install_root
            .join(CURRENT_MANIFEST)
            .to_string_lossy()
            .to_string(),
        rolled_back: true,
    })
}

pub(crate) fn cleanup_releases_after_startup() {
    if let Ok(root) = managed_install_root() {
        warn_if_release_gc_fails(&root);
    }
}

fn managed_install_root() -> Result<PathBuf, String> {
    let current = env::current_exe().map_err(|error| format!("解析当前 axon 路径失败: {error}"))?;
    let resolved = fs::canonicalize(&current)
        .map_err(|error| format!("解析当前 axon 实际路径失败: {error}"))?;
    let release_dir = resolved
        .parent()
        .ok_or_else(|| "当前 axon 路径缺少 release 目录".to_owned())?;
    let releases_dir = release_dir
        .parent()
        .ok_or_else(|| "当前 axon 路径不属于受管 release".to_owned())?;
    if releases_dir.file_name().and_then(|name| name.to_str()) != Some(VERSIONS_DIR) {
        return Err("当前 Axon 不位于受管 versions 目录，无法执行自升级".to_owned());
    }
    releases_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "当前 Axon 安装根目录无效".to_owned())
}

fn acquire_install_lock(root: &Path) -> Result<File, String> {
    fs::create_dir_all(root).map_err(|error| format!("创建安装根目录失败: {error}"))?;
    let lock_path = root.join(INSTALL_LOCK);
    let lock = File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| format!("打开安装锁失败: {error}"))?;
    lock.try_lock()
        .map_err(|_| "已有 Axon 安装、升级或回滚操作正在执行".to_owned())?;
    Ok(lock)
}

fn release_name(version: &str, _target: &str) -> Result<String, String> {
    let valid = |value: &str| {
        !value.is_empty()
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    };
    if !valid(version) {
        return Err("release 版本包含不安全字符".to_owned());
    }
    Ok(version.to_owned())
}

fn publish_release(
    archive: &Path,
    install_root: &Path,
    release_name: &str,
    release_dir: &Path,
) -> Result<(), String> {
    let releases_dir = install_root.join(VERSIONS_DIR);
    fs::create_dir_all(&releases_dir).map_err(|error| format!("创建 release 目录失败: {error}"))?;
    let staging = releases_dir.join(format!(".staging.{release_name}.{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|error| format!("清理旧 staging 目录失败: {error}"))?;
    }
    fs::create_dir_all(&staging).map_err(|error| format!("创建 staging 目录失败: {error}"))?;
    let extract_dir = staging.join("extract");
    if let Err(error) = extract_archive(archive, &extract_dir).and_then(|_| {
        let binary = find_binary_in_dir(&extract_dir)?;
        let package_root = binary
            .parent()
            .ok_or_else(|| "release 二进制缺少父目录".to_owned())?;
        fs::rename(package_root, release_dir)
            .map_err(|move_error| format!("发布 release 失败: {move_error}"))
    }) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| format!("创建解压目录失败: {error}"))?;
    let extension = archive
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let status = match extension {
        "gz" | "tgz" => non_interactive_command("tar")
            .args([
                "xzf",
                &archive.to_string_lossy(),
                "-C",
                &destination.to_string_lossy(),
            ])
            .status(),
        "zip" => extract_zip(archive, destination),
        other => return Err(format!("不支持的 archive 格式: .{other}")),
    }
    .map_err(|error| format!("解压 release 失败: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("解压 release 失败，exit={status}"))
}

#[cfg(windows)]
fn extract_zip(archive: &Path, destination: &Path) -> std::io::Result<std::process::ExitStatus> {
    non_interactive_command("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                archive.display(),
                destination.display()
            ),
        ])
        .status()
}

#[cfg(not(windows))]
fn extract_zip(archive: &Path, destination: &Path) -> std::io::Result<std::process::ExitStatus> {
    non_interactive_command("unzip")
        .args([
            "-q",
            "-o",
            &archive.to_string_lossy(),
            "-d",
            &destination.to_string_lossy(),
        ])
        .status()
}

fn find_binary_in_dir(dir: &Path) -> Result<PathBuf, String> {
    #[cfg(windows)]
    let binary_name = "axon.exe";
    #[cfg(not(windows))]
    let binary_name = "axon";
    let mut found = None;
    Walk::run(dir, &WalkOpts::default(), |entry| {
        if entry.kind == EntryKind::File
            && entry.path.file_name().and_then(|name| name.to_str()) == Some(binary_name)
        {
            found = Some(entry.path.clone());
            return WalkAction::Stop;
        }
        WalkAction::Continue
    })
    .map_err(|_| format!("解压后未找到 {binary_name}"))?;
    found.ok_or_else(|| format!("解压后未找到 {binary_name}"))
}

fn release_binary(release: &Path) -> Result<PathBuf, String> {
    let name = if cfg!(windows) { "axon.exe" } else { "axon" };
    let binary = release.join(name);
    binary
        .is_file()
        .then_some(binary)
        .ok_or_else(|| format!("release 缺少 {name}: {}", release.display()))
}

fn active_release(root: &Path) -> Result<PathBuf, String> {
    active_manifest_release(root, CURRENT_MANIFEST)
}

fn active_manifest_release(root: &Path, name: &str) -> Result<PathBuf, String> {
    let manifest: CurrentManifest = serde_json::from_slice(
        &fs::read(root.join(name)).map_err(|error| format!("读取 {name} 失败: {error}"))?,
    )
    .map_err(|error| format!("解析 {name} 失败: {error}"))?;
    if manifest.version_id.is_empty()
        || manifest.version_id.contains(['/', '\\'])
        || manifest.version_id.contains("..")
    {
        return Err(format!("{name} versionId 无效"));
    }
    let versions = root.join(VERSIONS_DIR);
    let release = versions.join(manifest.version_id);
    validate_managed_release_path(&release, &versions, name)?;
    release
        .is_dir()
        .then_some(release)
        .ok_or_else(|| format!("{name} 指向的版本不存在"))
}

fn activate_release(root: &Path, release: &Path, previous: Option<&Path>) -> Result<(), String> {
    let version_id = release
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && !name.contains(['/', '\\']))
        .ok_or_else(|| "版本目录名称无效".to_owned())?;
    if let Some(previous) = previous {
        let previous_id = previous
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "回滚版本目录名称无效".to_owned())?;
        write_manifest(root, PREVIOUS_MANIFEST, previous_id)?;
    }
    write_manifest(root, CURRENT_MANIFEST, version_id)
}

fn write_manifest(root: &Path, name: &str, version_id: &str) -> Result<(), String> {
    let manifest = serde_json::to_vec(&CurrentManifest {
        version_id: version_id.to_owned(),
    })
    .map_err(|error| format!("序列化 {name} 失败: {error}"))?;
    let temporary = root.join(format!(".{name}.{}.tmp", std::process::id()));
    fs::write(&temporary, manifest).map_err(|error| format!("写入 {name} 失败: {error}"))?;
    fs::rename(&temporary, root.join(name)).map_err(|error| format!("切换 {name} 失败: {error}"))
}

fn warn_if_release_gc_fails(root: &Path) {
    if let Err(error) = garbage_collect_releases(root) {
        axon_logger::warning(&format!("Axon release 回收已跳过: {error}"));
    }
}

fn restart_daemon_after_upgrade(root: &Path) -> Result<(), String> {
    let launcher = root.join("launcher").join(if cfg!(windows) {
        "axon-launcher.exe"
    } else {
        "axon-launcher"
    });
    if !launcher.is_file() {
        return Ok(());
    }
    let status = non_interactive_command(&launcher)
        .args(["daemon", "status"])
        .output()
        .map_err(|error| format!("读取升级前 Daemon 状态失败: {error}"))?;
    if !status.status.success() {
        return Err("读取升级前 Daemon 状态失败".to_owned());
    }
    let report: serde_json::Value = serde_json::from_slice(&status.stdout)
        .map_err(|error| format!("解析升级前 Daemon 状态失败: {error}"))?;
    if report.get("installed") != Some(&serde_json::Value::Bool(true)) {
        return Ok(());
    }
    let status = non_interactive_command(&launcher)
        .args(["daemon", "restart"])
        .status()
        .map_err(|error| format!("重启升级后的 Daemon 失败: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("重启升级后的 Daemon 失败，exit={status}"))
}

fn garbage_collect_releases(root: &Path) -> Result<(), String> {
    let releases_dir = root.join(VERSIONS_DIR);
    if !releases_dir.is_dir() {
        return Ok(());
    }
    let protected = protected_release_paths(root, &releases_dir)?;
    let entries =
        fs::read_dir(&releases_dir).map_err(|error| format!("读取 release 目录失败: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取 release 条目失败: {error}"))?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|error| format!("读取 release 条目类型失败: {error}"))?
            .is_dir()
            || entry.file_name().to_string_lossy().starts_with('.')
        {
            continue;
        }
        let canonical =
            fs::canonicalize(&path).map_err(|error| format!("解析 release 路径失败: {error}"))?;
        if !protected.contains(&canonical) {
            fs::remove_dir_all(&path).map_err(|error| format!("删除过期 release 失败: {error}"))?;
        }
    }
    let previous_manifest = root.join(PREVIOUS_MANIFEST);
    if previous_manifest.exists() {
        let remove_previous = active_manifest_release(root, PREVIOUS_MANIFEST)
            .map(|path| {
                fs::canonicalize(path)
                    .map(|canonical| !protected.contains(&canonical))
                    .unwrap_or(true)
            })
            .unwrap_or(true);
        if remove_previous {
            fs::remove_file(previous_manifest)
                .map_err(|error| format!("删除过期 previous manifest 失败: {error}"))?;
        }
    }
    Ok(())
}

fn protected_release_paths(root: &Path, releases_dir: &Path) -> Result<BTreeSet<PathBuf>, String> {
    let mut protected = BTreeSet::new();
    for manifest_name in [CURRENT_MANIFEST] {
        match active_manifest_release(root, manifest_name) {
            Ok(path) => {
                protected.insert(
                    fs::canonicalize(path)
                        .map_err(|error| format!("解析 {manifest_name} 版本失败: {error}"))?,
                );
            }
            Err(_error) if !root.join(manifest_name).exists() => {}
            Err(error) => return Err(error),
        }
    }
    let lease_dir = root.join(STATE_DIR).join(RUNTIME_RELEASE_LEASES_DIR);
    if !lease_dir.exists() {
        return Ok(protected);
    }
    let leases =
        fs::read_dir(&lease_dir).map_err(|error| format!("读取运行 release 租约失败: {error}"))?;
    for lease in leases {
        let lease = lease.map_err(|error| format!("读取运行 release 租约条目失败: {error}"))?;
        if !lease
            .file_type()
            .map_err(|error| format!("读取运行 release 租约类型失败: {error}"))?
            .is_file()
        {
            return Err("运行 release 租约目录包含非文件条目".to_owned());
        }
        let target = fs::read_to_string(lease.path())
            .map_err(|error| format!("读取运行 release 租约失败: {error}"))?;
        if target.trim().is_empty() {
            return Err("运行 release 租约为空".to_owned());
        }
        let pid = lease
            .path()
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse::<u32>().ok());
        if pid.is_some_and(|value| !release_owner_alive(value)) {
            fs::remove_file(lease.path())
                .map_err(|error| format!("删除失效运行 release 租约失败: {error}"))?;
            continue;
        }
        let release = fs::canonicalize(target.trim())
            .map_err(|error| format!("解析运行 release 租约失败: {error}"))?;
        validate_managed_release_path(&release, releases_dir, "运行租约")?;
        protected.insert(release);
    }
    Ok(protected)
}

fn release_owner_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(windows)]
    return non_interactive_command("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .ok()
        .is_some_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
        });
    #[cfg(unix)]
    return non_interactive_command("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success());
    #[cfg(not(any(windows, unix)))]
    true
}

fn validate_managed_release_path(
    release: &Path,
    releases_dir: &Path,
    source: &str,
) -> Result<(), String> {
    let expected_parent = fs::canonicalize(releases_dir)
        .map_err(|error| format!("解析受管 versions 目录失败: {error}"))?;
    let parent = release
        .parent()
        .ok_or_else(|| format!("{source} release 路径无父目录"))?;
    if fs::canonicalize(parent).ok().as_deref() != Some(expected_parent.as_path()) {
        return Err(format!("{source} release 路径不属于受管 releases 目录"));
    }
    Ok(())
}

fn smoke_test(binary: &Path) -> bool {
    non_interactive_command(binary)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_name_rejects_unsafe_values() {
        assert_eq!(
            release_name("1.2.3", "x86_64-pc-windows-msvc"),
            Ok("1.2.3".to_owned())
        );
        assert!(release_name("1/2", "target").is_err());
    }

    #[test]
    fn manifest_switches_to_new_target() {
        let root = axon_test_support::test_temp_dir();
        let versions = root.path().join(VERSIONS_DIR);
        let first = versions.join("first");
        let second = versions.join("second");
        fs::create_dir_all(&first).expect("首个版本目录应可创建");
        fs::create_dir_all(&second).expect("第二个版本目录应可创建");
        activate_release(root.path(), &first, None).expect("首个版本应可发布");
        activate_release(root.path(), &second, Some(&first)).expect("第二个版本应可发布");
        assert_eq!(
            active_release(root.path()).expect("当前版本应可解析"),
            second
        );
        assert_eq!(
            active_manifest_release(root.path(), PREVIOUS_MANIFEST).expect("回滚版本应可解析"),
            first
        );
    }

    #[test]
    fn release_gc_preserves_active_and_leased_releases() {
        let root = axon_test_support::test_temp_dir();
        let releases = root.path().join(VERSIONS_DIR);
        let current = create_release(&releases, "current-release");
        let previous = create_release(&releases, "previous-release");
        let leased = create_release(&releases, "leased-release");
        let obsolete = create_release(&releases, "obsolete-release");
        activate_release(root.path(), &current, None).expect("设置 current");
        activate_release(root.path(), &previous, Some(&current)).expect("设置 previous");
        let lease_dir = root.path().join(STATE_DIR).join(RUNTIME_RELEASE_LEASES_DIR);
        fs::create_dir_all(&lease_dir).expect("创建租约目录");
        fs::write(
            lease_dir.join(format!("{}.lease", std::process::id())),
            leased.to_string_lossy().as_bytes(),
        )
        .expect("写入租约");

        garbage_collect_releases(root.path()).expect("回收过期 release");

        assert!(!current.is_dir());
        assert!(previous.is_dir());
        assert!(leased.is_dir());
        assert!(!obsolete.exists());
        assert!(!root.path().join(PREVIOUS_MANIFEST).exists());
    }

    #[test]
    fn release_gc_fails_closed_when_lease_is_invalid() {
        let root = axon_test_support::test_temp_dir();
        let releases = root.path().join(VERSIONS_DIR);
        let current = create_release(&releases, "current-release");
        let obsolete = create_release(&releases, "obsolete-release");
        activate_release(root.path(), &current, None).expect("设置 current");
        let lease_dir = root.path().join(STATE_DIR).join(RUNTIME_RELEASE_LEASES_DIR);
        fs::create_dir_all(&lease_dir).expect("创建租约目录");
        fs::write(lease_dir.join("42.lease"), b"").expect("写入无效租约");

        assert!(garbage_collect_releases(root.path()).is_err());
        assert!(obsolete.exists());
    }

    fn create_release(releases: &Path, name: &str) -> PathBuf {
        let release = releases.join(name);
        fs::create_dir_all(&release).expect("创建 release");
        release
    }
}
