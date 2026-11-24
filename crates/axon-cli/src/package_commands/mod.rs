//! `axon package` portable release 包生成入口。

mod package_archive;
mod package_install;

use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_crypto::sha256_hex;
use axon_process::non_interactive_command;
use serde::{Deserialize, Serialize};

use package_archive::{PackageArchiveReport, create_portable_zip_archive};
use package_install::create_install_script;

use crate::{CliRunResult, OutputFormat, PackageCommand};

const DEFAULT_OUTPUT_DIR: &str = "dist";
const PACKAGE_MANIFEST_FILE: &str = "axon-release-manifest.json";
const PACKAGE_STATUS_READY: &str = "package_ready";
const PACKAGE_STATUS_VERIFIED: &str = "package_verified";
const PACKAGE_KIND_PORTABLE: &str = "portable_directory";
const PACKAGE_TOOL_NAME: &str = "axon.package";
const PACKAGE_BIN_DIR: &str = "bin";
const PACKAGE_BINARY_STEM: &str = "axon";
const PACKAGE_BINARY_WINDOWS: &str = "axon.exe";
const PACKAGE_LAUNCHER_UNIX: &str = "axon-launcher";
const PACKAGE_LAUNCHER_WINDOWS: &str = "axon-launcher.exe";
const PACKAGE_DEFAULTS_DIR: &str = "defaults";
const PACKAGE_DEFAULT_SOUL_FILE: &str = "SOUL.md";
const DEFAULT_SOUL_CONTENT: &str = include_str!("../../../../assets/defaults/SOUL.md");

/// 执行 `axon package`。
#[must_use]
pub fn run_package(command: &PackageCommand, format: OutputFormat) -> CliRunResult {
    let result = if let Some(manifest_path) = command.verify_manifest.as_ref() {
        PackageVerifyPlan::new(manifest_path.clone(), command.no_smoke).execute()
    } else {
        PackagePlan::from_command(command).and_then(PackagePlan::execute)
    };

    match result {
        Ok(report) => success_result(&report, format),
        Err(message) => CliRunResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: format!("{message}\n"),
        },
    }
}

struct PackagePlan {
    target: String,
    binary: PathBuf,
    output_dir: PathBuf,
    archive: bool,
}

impl PackagePlan {
    fn from_command(command: &PackageCommand) -> Result<Self, String> {
        let target = command.target.clone().unwrap_or_else(current_target_triple);
        validate_target(&target)?;
        let binary = command.binary.clone().unwrap_or_else(default_binary_path);
        let output_dir = command
            .output_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT_DIR));
        Ok(Self {
            target,
            binary,
            output_dir,
            archive: command.archive,
        })
    }

    fn execute(self) -> Result<PackageReport, String> {
        if !self.binary.is_file() {
            return Err(format!(
                "package binary 不存在；请先构建 release 二进制或传入 --binary: {}",
                self.binary.display()
            ));
        }

        let version = env!("CARGO_PKG_VERSION").to_owned();
        let package_name = format!("axon-{version}-{}", self.target);
        let package_dir = self.output_dir.join(&package_name);
        let bin_dir = package_dir.join(PACKAGE_BIN_DIR);
        fs::create_dir_all(&bin_dir).map_err(|error| format!("创建 package 目录失败: {error}"))?;

        let binary_name = package_binary_name(&self.target);
        let packaged_binary = bin_dir.join(binary_name);
        if packaged_binary.exists() {
            return Err(format!(
                "package 目标二进制已存在: {}",
                packaged_binary.display()
            ));
        }
        fs::copy(&self.binary, &packaged_binary)
            .map_err(|error| format!("复制 package 二进制失败: {error}"))?;
        let launcher_name = package_launcher_name(&self.target);
        let launcher_source = self
            .binary
            .parent()
            .map(|parent| parent.join(launcher_name))
            .ok_or_else(|| "package 二进制缺少 launcher 目录".to_owned())?;
        if !launcher_source.is_file() {
            return Err(format!(
                "package 缺少稳定 launcher，请先构建 {launcher_name}: {}",
                launcher_source.display()
            ));
        }
        fs::copy(&launcher_source, bin_dir.join(launcher_name))
            .map_err(|error| format!("复制 package launcher 失败: {error}"))?;

        let defaults_dir = package_dir.join(PACKAGE_DEFAULTS_DIR);
        fs::create_dir_all(&defaults_dir)
            .map_err(|error| format!("创建 package defaults 目录失败: {error}"))?;
        fs::write(
            defaults_dir.join(PACKAGE_DEFAULT_SOUL_FILE),
            DEFAULT_SOUL_CONTENT,
        )
        .map_err(|error| format!("写入 package 默认 SOUL.md 失败: {error}"))?;

        let binary_bytes = fs::read(&packaged_binary)
            .map_err(|error| format!("读取 package 二进制失败: {error}"))?;
        let binary_sha256 = sha256_hex(&binary_bytes);
        let install_script = create_install_script(&package_dir, &self.target)?;
        let install_hint = install_hint(&self.target, &packaged_binary);
        let smoke_command = format!("{} --version", packaged_binary.display());
        let manifest = PackageManifest {
            schema_version: 1,
            package: package_name.clone(),
            version: version.clone(),
            target: self.target.clone(),
            kind: PACKAGE_KIND_PORTABLE.to_owned(),
            binary: packaged_binary.to_string_lossy().to_string(),
            binary_sha256: binary_sha256.clone(),
            install_script: install_script.path.to_string_lossy().to_string(),
            install_script_sha256: install_script.sha256.clone(),
            install_hint: install_hint.clone(),
            smoke_command: smoke_command.clone(),
            manual_install_only: true,
            auto_install_allowed: false,
            self_replace_allowed: false,
            remote_trigger_allowed: false,
            network_publish_allowed: false,
        };
        let manifest_path = package_dir.join(PACKAGE_MANIFEST_FILE);
        write_manifest(&manifest_path, &manifest)?;
        let archive_report = if self.archive {
            Some(create_portable_zip_archive(
                &package_dir,
                &self.output_dir,
                &package_name,
            )?)
        } else {
            None
        };

        Ok(PackageReport {
            status: PACKAGE_STATUS_READY,
            tool: PACKAGE_TOOL_NAME,
            package: package_name,
            version,
            target: self.target,
            kind: PACKAGE_KIND_PORTABLE,
            package_dir: package_dir.to_string_lossy().to_string(),
            manifest_path: manifest_path.to_string_lossy().to_string(),
            binary_path: packaged_binary.to_string_lossy().to_string(),
            binary_sha256,
            install_script_path: install_script.path.to_string_lossy().to_string(),
            install_script_sha256: install_script.sha256,
            install_hint,
            smoke_command,
            smoke_checked: false,
            smoke_passed: false,
            archive: archive_report.map(PackageArchiveSummary::from),
            manual_install_only: true,
            auto_install_allowed: false,
            self_replace_allowed: false,
            remote_trigger_allowed: false,
            network_publish_allowed: false,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageReport {
    status: &'static str,
    tool: &'static str,
    package: String,
    version: String,
    target: String,
    kind: &'static str,
    package_dir: String,
    manifest_path: String,
    binary_path: String,
    binary_sha256: String,
    install_script_path: String,
    install_script_sha256: String,
    install_hint: String,
    smoke_command: String,
    smoke_checked: bool,
    smoke_passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    archive: Option<PackageArchiveSummary>,
    manual_install_only: bool,
    auto_install_allowed: bool,
    self_replace_allowed: bool,
    remote_trigger_allowed: bool,
    network_publish_allowed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageArchiveSummary {
    #[serde(rename = "archivePath")]
    path: String,
    #[serde(rename = "archiveSha256")]
    sha256: String,
    #[serde(rename = "archiveBytes")]
    bytes: u64,
    #[serde(rename = "archiveSidecarPath")]
    sidecar_path: String,
    #[serde(rename = "archiveFormat")]
    format: &'static str,
}

impl From<PackageArchiveReport> for PackageArchiveSummary {
    fn from(report: PackageArchiveReport) -> Self {
        Self {
            path: report.path.to_string_lossy().to_string(),
            sha256: report.sha256,
            bytes: report.bytes,
            sidecar_path: report.sidecar_path.to_string_lossy().to_string(),
            format: report.format,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageManifest {
    schema_version: u32,
    package: String,
    version: String,
    target: String,
    kind: String,
    binary: String,
    binary_sha256: String,
    install_script: String,
    install_script_sha256: String,
    install_hint: String,
    smoke_command: String,
    manual_install_only: bool,
    auto_install_allowed: bool,
    self_replace_allowed: bool,
    remote_trigger_allowed: bool,
    network_publish_allowed: bool,
}

fn success_result(report: &PackageReport, format: OutputFormat) -> CliRunResult {
    match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: report.to_text(),
            stderr: String::new(),
        },
        OutputFormat::Json => match serde_json::to_string(report) {
            Ok(line) => CliRunResult {
                exit_code: 0,
                stdout: format!("{line}\n"),
                stderr: String::new(),
            },
            Err(error) => CliRunResult {
                exit_code: 2,
                stdout: String::new(),
                stderr: format!("package report JSON 序列化失败: {error}\n"),
            },
        },
    }
}

impl PackageReport {
    fn to_text(&self) -> String {
        let smoke = if self.smoke_checked {
            format!("\nsmoke_passed: {}", self.smoke_passed)
        } else {
            String::new()
        };
        let archive = self.archive.as_ref().map_or_else(String::new, |archive| {
            format!(
                "\narchive: {}\narchive_sha256: {}",
                archive.path, archive.sha256
            )
        });
        format!(
            "Axon package {}: {}\ntarget: {}\nmanifest: {}\nsha256: {}\ninstall_script: {}\nsmoke: {}{}{}\n",
            self.status.trim_start_matches("package_"),
            self.package,
            self.target,
            self.manifest_path,
            self.binary_sha256,
            self.install_script_path,
            self.smoke_command,
            smoke,
            archive
        )
    }
}

fn write_manifest(path: &Path, manifest: &PackageManifest) -> Result<(), String> {
    let text = serde_json::to_string_pretty(manifest)
        .map_err(|error| format!("package manifest 序列化失败: {error}"))?;
    fs::write(path, text).map_err(|error| format!("写入 package manifest 失败: {error}"))
}

struct PackageVerifyPlan {
    manifest_path: PathBuf,
    no_smoke: bool,
}

impl PackageVerifyPlan {
    fn new(manifest_path: PathBuf, no_smoke: bool) -> Self {
        Self {
            manifest_path,
            no_smoke,
        }
    }

    fn execute(self) -> Result<PackageReport, String> {
        let manifest_text = fs::read_to_string(&self.manifest_path)
            .map_err(|error| format!("读取 package manifest 失败: {error}"))?;
        let manifest: PackageManifest = serde_json::from_str(&manifest_text)
            .map_err(|error| format!("解析 package manifest 失败: {error}"))?;
        verify_manifest_contract(&manifest)?;

        let binary_path = PathBuf::from(&manifest.binary);
        let binary_bytes =
            fs::read(&binary_path).map_err(|error| format!("读取 package 二进制失败: {error}"))?;
        let actual_sha256 = sha256_hex(&binary_bytes);
        if actual_sha256 != manifest.binary_sha256 {
            return Err(format!(
                "package binary sha256 不匹配；expected={}, actual={}",
                manifest.binary_sha256, actual_sha256
            ));
        }
        let install_script_bytes = fs::read(&manifest.install_script)
            .map_err(|error| format!("读取 package 安装脚本失败: {error}"))?;
        let actual_install_script_sha256 = sha256_hex(&install_script_bytes);
        if actual_install_script_sha256 != manifest.install_script_sha256 {
            return Err(format!(
                "package 安装脚本 sha256 不匹配；expected={}, actual={}",
                manifest.install_script_sha256, actual_install_script_sha256
            ));
        }

        let smoke_checked = !self.no_smoke;
        let smoke_passed = if smoke_checked {
            run_version_smoke(&binary_path)?
        } else {
            false
        };
        let package_dir = self
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_string_lossy()
            .to_string();

        Ok(PackageReport {
            status: PACKAGE_STATUS_VERIFIED,
            tool: PACKAGE_TOOL_NAME,
            package: manifest.package,
            version: manifest.version,
            target: manifest.target,
            kind: PACKAGE_KIND_PORTABLE,
            package_dir,
            manifest_path: self.manifest_path.to_string_lossy().to_string(),
            binary_path: binary_path.to_string_lossy().to_string(),
            binary_sha256: actual_sha256,
            install_script_path: manifest.install_script,
            install_script_sha256: actual_install_script_sha256,
            install_hint: manifest.install_hint,
            smoke_command: manifest.smoke_command,
            smoke_checked,
            smoke_passed,
            archive: None,
            manual_install_only: true,
            auto_install_allowed: false,
            self_replace_allowed: false,
            remote_trigger_allowed: false,
            network_publish_allowed: false,
        })
    }
}

fn verify_manifest_contract(manifest: &PackageManifest) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err("package manifest schemaVersion 必须为 1".to_owned());
    }
    validate_target(&manifest.target)?;
    if manifest.kind != PACKAGE_KIND_PORTABLE {
        return Err("package manifest kind 必须为 portable_directory".to_owned());
    }
    if !manifest.manual_install_only {
        return Err("package manifest 必须声明 manualInstallOnly=true".to_owned());
    }
    if manifest.install_script.trim().is_empty() || manifest.install_script_sha256.trim().is_empty()
    {
        return Err("package manifest 必须包含安装脚本路径和 sha256".to_owned());
    }
    if manifest.auto_install_allowed
        || manifest.self_replace_allowed
        || manifest.remote_trigger_allowed
        || manifest.network_publish_allowed
    {
        return Err("package manifest 安全标记必须全部保持关闭".to_owned());
    }
    Ok(())
}

fn run_version_smoke(binary_path: &Path) -> Result<bool, String> {
    let status = non_interactive_command(binary_path)
        .arg("--version")
        .status()
        .map_err(|error| format!("执行 package smoke 失败: {error}"))?;
    if status.success() {
        Ok(true)
    } else {
        Err(format!("package smoke 失败；exit={status}"))
    }
}

fn validate_target(target: &str) -> Result<(), String> {
    let valid = !target.trim().is_empty()
        && target
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err("package target 只能包含 ASCII 字母、数字、点、下划线或横线".to_owned())
    }
}

fn default_binary_path() -> PathBuf {
    let filename = package_binary_name(&current_target_triple());
    PathBuf::from("target").join("release").join(filename)
}

fn package_binary_name(target: &str) -> &'static str {
    if target.contains("windows") || target.contains("pc-windows") {
        PACKAGE_BINARY_WINDOWS
    } else {
        PACKAGE_BINARY_STEM
    }
}

fn package_launcher_name(target: &str) -> &'static str {
    if target.contains("windows") || target.contains("pc-windows") {
        PACKAGE_LAUNCHER_WINDOWS
    } else {
        PACKAGE_LAUNCHER_UNIX
    }
}

fn install_hint(target: &str, binary_path: &Path) -> String {
    if target.contains("windows") {
        format!(
            "运行同目录生成的 install.ps1；它会自动将 bin 与 launcher 加入用户 PATH（package 二进制：{}）",
            binary_path.display()
        )
    } else {
        format!(
            "运行同目录生成的 install.sh；它会自动将 CLI 与 launcher 加入 shell PATH（package 二进制：{}）",
            binary_path.display()
        )
    }
}

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

#[cfg(test)]
#[path = "../tests/package_commands_tests.rs"]
mod tests;
