//! `axon upgrade` 更新治理入口。

mod runtime_release_lease;
mod startup_update;
mod upgrade_apply;

use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

use axon_crypto::sha256_hex;
use axon_rustls_provider::ensure_rustls_crypto_provider;
use serde::{Deserialize, Serialize};

use crate::{CliRunResult, OutputFormat, UpgradeCommand};

pub(crate) use runtime_release_lease::acquire_runtime_release_lease;
pub(crate) use startup_update::{StartupUpdateGate, run_startup_update_gate};
pub(crate) use upgrade_apply::cleanup_releases_after_startup;
use upgrade_apply::{apply_upgrade, rollback_upgrade};

const DEFAULT_RELEASE_MANIFEST_URL: &str =
    "https://api.github.com/repos/a574676848/axon-code/releases/latest";
const HTTP_TIMEOUT_SECS: u64 = 30;
const USER_AGENT: &str = "axon-cli-upgrade";

/// 执行 `axon upgrade`。
#[must_use]
pub fn run_upgrade(command: &UpgradeCommand, format: OutputFormat) -> CliRunResult {
    // 处理 rollback
    if command.rollback {
        return match perform_rollback() {
            Ok(report) => success_result(&report, format),
            Err(message) => CliRunResult {
                exit_code: 2,
                stdout: String::new(),
                stderr: format!("{message}\n"),
            },
        };
    }

    match UpgradePlan::from_command(command).and_then(UpgradePlan::execute) {
        Ok(report) => success_result(&report, format),
        Err(message) => CliRunResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: format!("{message}\n"),
        },
    }
}

pub(super) struct UpgradePlan {
    manifest_url: String,
    download: bool,
    apply: bool,
    output_dir: Option<PathBuf>,
    asset: Option<String>,
}

impl UpgradePlan {
    pub(super) fn from_command(command: &UpgradeCommand) -> Result<Self, String> {
        let manifest_url = command
            .manifest_url
            .clone()
            .unwrap_or_else(|| DEFAULT_RELEASE_MANIFEST_URL.to_owned());
        if !manifest_url.starts_with("https://") && !manifest_url.starts_with("http://") {
            return Err("upgrade manifest-url 只允许 http/https URL".to_owned());
        }

        // 检查互斥参数
        let mut flags_count = 0;
        if command.download {
            flags_count += 1;
        }
        if command.apply {
            flags_count += 1;
        }
        if command.rollback {
            flags_count += 1;
        }

        if flags_count > 1 {
            return Err("--download、--apply 和 --rollback 互斥，请只指定其中一个".to_owned());
        }

        Ok(Self {
            manifest_url,
            download: command.download,
            apply: command.apply,
            output_dir: command.output_dir.clone(),
            asset: command.asset.clone(),
        })
    }

    pub(super) fn execute(self) -> Result<UpgradeReport, String> {
        let manifest = fetch_release_manifest(&self.manifest_url)?;
        let latest_version = normalize_version(&manifest.tag_name);
        let current_version = env!("CARGO_PKG_VERSION").to_owned();
        let update_available = is_newer_version(&latest_version, &current_version);

        if self.apply {
            // --apply：下载 → SHA256 校验 → 原子替换 → 回滚保护
            if !update_available {
                return Ok(UpgradeReport {
                    status: "upgrade_already_latest",
                    current_version,
                    latest_version,
                    update_available: false,
                    release_url: manifest.html_url,
                    asset_count: manifest.assets.len(),
                    download: None,
                    apply_result: None,
                    auto_install_allowed: true,
                    self_replace_allowed: false,
                    remote_trigger_allowed: false,
                    raw_manifest_persisted: false,
                });
            }
            let target = current_target_triple();
            let asset_name = select_platform_asset_name(&manifest.assets, &target)?;
            let dl = download_to_temp(&manifest.assets, Some(asset_name.as_str()))?;
            let apply_result = apply_upgrade(
                &dl.downloaded_path,
                &dl.downloaded_sha256,
                &latest_version,
                &target,
            )?;
            return Ok(UpgradeReport {
                status: "upgrade_applied",
                current_version,
                latest_version,
                update_available,
                release_url: manifest.html_url,
                asset_count: manifest.assets.len(),
                download: Some(dl),
                apply_result: Some(apply_result),
                auto_install_allowed: true,
                self_replace_allowed: true,
                remote_trigger_allowed: false,
                raw_manifest_persisted: false,
            });
        }

        let download = if self.download {
            Some(download_asset(
                &manifest.assets,
                self.asset.as_deref(),
                self.output_dir.as_deref(),
            )?)
        } else {
            None
        };

        Ok(UpgradeReport {
            status: if download.is_some() {
                "upgrade_downloaded"
            } else {
                "upgrade_checked"
            },
            current_version,
            latest_version,
            update_available,
            release_url: manifest.html_url,
            asset_count: manifest.assets.len(),
            download,
            apply_result: None,
            auto_install_allowed: false,
            self_replace_allowed: false,
            remote_trigger_allowed: false,
            raw_manifest_persisted: false,
        })
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct ReleaseManifest {
    tag_name: String,
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ReleaseAsset {
    pub(super) name: String,
    pub(super) browser_download_url: String,
    pub(super) size: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpgradeReport {
    pub(super) status: &'static str,
    pub(super) current_version: String,
    pub(super) latest_version: String,
    pub(super) update_available: bool,
    pub(super) release_url: Option<String>,
    pub(super) asset_count: usize,
    pub(super) download: Option<UpgradeDownloadReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) apply_result: Option<UpgradeApplyReport>,
    pub(super) auto_install_allowed: bool,
    pub(super) self_replace_allowed: bool,
    pub(super) remote_trigger_allowed: bool,
    pub(super) raw_manifest_persisted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpgradeDownloadReport {
    pub(super) asset_name: String,
    pub(super) asset_size: Option<u64>,
    pub(super) downloaded_path: String,
    pub(super) downloaded_sha256: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpgradeApplyReport {
    pub(super) backup_path: String,
    pub(super) install_path: String,
    pub(super) rolled_back: bool,
}

pub(super) fn fetch_release_manifest(url: &str) -> Result<ReleaseManifest, String> {
    ensure_rustls_crypto_provider();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(HTTP_TIMEOUT_SECS)))
        .build()
        .into();
    let body = agent
        .get(url)
        .header("user-agent", USER_AGENT)
        .call()
        .map_err(|error| format!("获取 upgrade manifest 失败: {error}"))?
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("读取 upgrade manifest 失败: {error}"))?;
    serde_json::from_str(&body).map_err(|error| format!("解析 upgrade manifest 失败: {error}"))
}

/// 根据当前平台 target triple 在 release assets 中匹配最合适的 asset 名称。
fn select_platform_asset_name(assets: &[ReleaseAsset], target: &str) -> Result<String, String> {
    // 优先精确匹配包含 target triple 的 asset
    if let Some(asset) = assets.iter().find(|a| a.name.contains(target)) {
        return Ok(asset.name.clone());
    }
    // 兜底：单 asset release 直接使用
    match assets {
        [single] => Ok(single.name.clone()),
        [] => Err("当前 release 没有可下载 asset".to_owned()),
        _ => Err(format!(
            "未能自动匹配平台 {target} 的 asset，请通过 --asset 明确指定"
        )),
    }
}

/// 下载 asset 到系统临时目录（用于 --apply 流程）。
fn download_to_temp(
    assets: &[ReleaseAsset],
    selected_name: Option<&str>,
) -> Result<UpgradeDownloadReport, String> {
    let asset = select_asset(assets, selected_name)?;
    if !asset.browser_download_url.starts_with("https://")
        && !asset.browser_download_url.starts_with("http://")
    {
        return Err("release asset 下载地址只允许 http/https URL".to_owned());
    }
    let filename = safe_asset_filename(&asset.name)?;
    let dir = std::env::temp_dir().join("axon-upgrade");
    fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let path = dir.join(filename);

    let bytes = download_bytes(&asset.browser_download_url)?;
    // 临时目录允许覆盖旧下载
    fs::write(&path, &bytes).map_err(|e| format!("写入临时文件失败: {e}"))?;

    Ok(UpgradeDownloadReport {
        asset_name: asset.name.clone(),
        asset_size: asset.size,
        downloaded_path: path.to_string_lossy().to_string(),
        downloaded_sha256: sha256_hex(&bytes),
    })
}

fn download_asset(
    assets: &[ReleaseAsset],
    selected_name: Option<&str>,
    output_dir: Option<&Path>,
) -> Result<UpgradeDownloadReport, String> {
    let asset = select_asset(assets, selected_name)?;
    if !asset.browser_download_url.starts_with("https://")
        && !asset.browser_download_url.starts_with("http://")
    {
        return Err("release asset 下载地址只允许 http/https URL".to_owned());
    }
    let filename = safe_asset_filename(&asset.name)?;
    let dir = output_dir
        .map(Path::to_path_buf)
        .map_or_else(std::env::current_dir, Ok)
        .map_err(|error| format!("解析下载目录失败: {error}"))?;
    fs::create_dir_all(&dir).map_err(|error| format!("创建下载目录失败: {error}"))?;
    let path = dir.join(filename);

    let bytes = download_bytes(&asset.browser_download_url)?;
    write_new_file(&path, &bytes)?;

    Ok(UpgradeDownloadReport {
        asset_name: asset.name.clone(),
        asset_size: asset.size,
        downloaded_path: path.to_string_lossy().to_string(),
        downloaded_sha256: sha256_hex(&bytes),
    })
}

fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    ensure_rustls_crypto_provider();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(HTTP_TIMEOUT_SECS)))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("user-agent", USER_AGENT)
        .call()
        .map_err(|error| format!("下载 release asset 失败: {error}"))?;
    let mut bytes = Vec::new();
    let mut reader = response.body_mut().as_reader();
    std::io::Read::read_to_end(&mut reader, &mut bytes)
        .map_err(|error| format!("读取 release asset 失败: {error}"))?;
    Ok(bytes)
}

fn select_asset<'a>(
    assets: &'a [ReleaseAsset],
    selected_name: Option<&str>,
) -> Result<&'a ReleaseAsset, String> {
    if let Some(name) = selected_name {
        return assets
            .iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| format!("未找到指定 release asset: {name}"));
    }
    match assets {
        [asset] => Ok(asset),
        [] => Err("当前 release 没有可下载 asset".to_owned()),
        _ => Err("release 包含多个 asset，请通过 --asset 明确选择".to_owned()),
    }
}

fn safe_asset_filename(name: &str) -> Result<&str, String> {
    let is_safe = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'));
    if is_safe {
        Ok(name)
    } else {
        Err("release asset 文件名包含不安全字符".to_owned())
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        return Err(format!("下载目标已存在: {}", path.display()));
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("创建下载文件失败: {error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("写入下载文件失败: {error}"))
}

pub(super) fn normalize_version(version: &str) -> String {
    version.trim().trim_start_matches('v').to_owned()
}

pub(super) fn is_newer_version(latest: &str, current: &str) -> bool {
    version_parts(latest) > version_parts(current)
}

fn version_parts(version: &str) -> Vec<u64> {
    version
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

/// 返回当前进程的 Rust target triple（运行时检测）。
fn current_target_triple() -> String {
    let arch = std::env::consts::ARCH;
    if cfg!(windows) {
        format!("{arch}-pc-windows-msvc")
    } else if cfg!(target_os = "macos") {
        format!("{arch}-apple-darwin")
    } else if cfg!(target_os = "linux") {
        format!("{arch}-unknown-linux-gnu")
    } else {
        format!("{arch}-unknown-{}", std::env::consts::OS)
    }
}

fn success_result(report: &UpgradeReport, format: OutputFormat) -> CliRunResult {
    match format {
        OutputFormat::Json => match serde_json::to_string(report) {
            Ok(line) => CliRunResult {
                exit_code: 0,
                stdout: format!("{line}\n"),
                stderr: String::new(),
            },
            Err(error) => CliRunResult {
                exit_code: 2,
                stdout: String::new(),
                stderr: format!("upgrade report JSON 序列化失败: {error}\n"),
            },
        },
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: text_report(report),
            stderr: String::new(),
        },
    }
}

fn text_report(report: &UpgradeReport) -> String {
    let update = if report.update_available {
        "可更新"
    } else {
        "已是最新"
    };
    let mut output = format!(
        "Axon upgrade: {update}\ncurrent: {}\nlatest: {}\n",
        report.current_version, report.latest_version
    );
    if let Some(download) = &report.download {
        let _ = write!(
            output,
            "downloaded: {}\nsha256: {}\n",
            download.downloaded_path, download.downloaded_sha256
        );
    }
    if let Some(apply) = &report.apply_result {
        if apply.rolled_back {
            let _ = writeln!(output, "apply: 失败，已回滚至 {}", apply.backup_path);
        } else {
            let _ = write!(
                output,
                "apply: 成功\ninstalled: {}\nbackup: {}\n",
                apply.install_path, apply.backup_path
            );
        }
    } else {
        let self_replace = if report.self_replace_allowed {
            "enabled"
        } else {
            "disabled"
        };
        let _ = writeln!(output, "self replace: {self_replace}");
    }
    output
}

/// 切换到上一个已验证 release。
fn perform_rollback() -> Result<UpgradeReport, String> {
    let apply_result = rollback_upgrade()?;
    Ok(UpgradeReport {
        status: "upgrade_rolled_back",
        current_version: "rolled_back".to_owned(),
        latest_version: "N/A".to_owned(),
        update_available: false,
        release_url: None,
        asset_count: 0,
        download: None,
        apply_result: Some(apply_result),
        auto_install_allowed: false,
        self_replace_allowed: false,
        remote_trigger_allowed: false,
        raw_manifest_persisted: false,
    })
}
