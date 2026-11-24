//! portable package 的本机安装脚本生成。

use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_crypto::sha256_hex;

const WINDOWS_INSTALL_SCRIPT: &str = "install.ps1";
const UNIX_INSTALL_SCRIPT: &str = "install.sh";

#[derive(Debug)]
pub(super) struct PackageInstallScript {
    pub(super) path: PathBuf,
    pub(super) sha256: String,
}

pub(super) fn create_install_script(
    package_dir: &Path,
    target: &str,
) -> Result<PackageInstallScript, String> {
    let (script_name, content) = if target.contains("windows") || target.contains("pc-windows") {
        (WINDOWS_INSTALL_SCRIPT, windows_install_script())
    } else {
        (UNIX_INSTALL_SCRIPT, unix_install_script())
    };
    let path = package_dir.join(script_name);
    fs::write(&path, content).map_err(|error| format!("写入 package 安装脚本失败: {error}"))?;
    let bytes = fs::read(&path).map_err(|error| format!("读取 package 安装脚本失败: {error}"))?;

    Ok(PackageInstallScript {
        path,
        sha256: sha256_hex(&bytes),
    })
}

fn windows_install_script() -> &'static str {
    r#"param(
    [string]$InstallDir = ""
)

$ErrorActionPreference = "Stop"
$Source = Join-Path $PSScriptRoot "bin\axon.exe"
$LauncherSource = Join-Path $PSScriptRoot "bin\axon-launcher.exe"
$Manifest = Join-Path $PSScriptRoot "axon-release-manifest.json"
$AxonUserDir = Join-Path $HOME ".axon"
$SoulSource = Join-Path $PSScriptRoot "defaults\SOUL.md"
$SoulFile = Join-Path $AxonUserDir "SOUL.md"
$IdentityDir = Join-Path $AxonUserDir "identity"
$OwnerFile = Join-Path $IdentityDir "owner.json"

if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
    throw "Axon binary not found: $Source"
}
if (-not (Test-Path -LiteralPath $LauncherSource -PathType Leaf)) {
    throw "Axon launcher not found: $LauncherSource"
}
$VersionId = (Get-Content -Raw $Manifest | ConvertFrom-Json).version
$Target = (Get-Content -Raw $Manifest | ConvertFrom-Json).target
if ([string]::IsNullOrWhiteSpace($Target)) {
    throw "package 版本清单缺少 target"
}

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = Join-Path $HOME ".axon\bin"
}

if ([string]::IsNullOrWhiteSpace($env:AXON_INSTALL_ROOT)) {
    $InstallRoot = Join-Path $HOME ".axon\bin"
} else {
    $InstallRoot = $env:AXON_INSTALL_ROOT
}
$ReleaseName = "$VersionId"
$VersionsDir = Join-Path $InstallRoot "versions"
$VersionDir = Join-Path $VersionsDir $ReleaseName
$LauncherDir = Join-Path $InstallRoot "launcher"
$LauncherPath = Join-Path $LauncherDir "axon-launcher.exe"
$StagingDir = Join-Path $VersionsDir ".staging.$ReleaseName.$PID"
New-Item -ItemType Directory -Force -Path $VersionsDir, $LauncherDir, $InstallDir | Out-Null
if (Test-Path -LiteralPath $LauncherPath -PathType Leaf) {
    & $LauncherPath daemon stop *> $null
}
Get-Process axon-desktop -ErrorAction SilentlyContinue | Where-Object {
    $_.Path -and $_.Path.StartsWith("$InstallRoot\", [StringComparison]::OrdinalIgnoreCase)
} | Stop-Process -Force
New-Item -ItemType Directory -Force -Path $StagingDir | Out-Null
$StagedSource = Join-Path $StagingDir "axon.exe"
Copy-Item -LiteralPath $Source -Destination $StagedSource
& $StagedSource --version *> $null
if ($LASTEXITCODE -ne 0) {
    throw "staging 版本无法通过 axon --version 验证"
}
if (Test-Path -LiteralPath $VersionDir -PathType Container) {
    Remove-Item -LiteralPath $VersionDir -Recurse -Force
}
Move-Item -LiteralPath $StagingDir -Destination $VersionDir
Copy-Item -LiteralPath $LauncherSource -Destination $LauncherPath -Force
Get-ChildItem -LiteralPath $VersionsDir -Directory -ErrorAction SilentlyContinue |
    Where-Object { -not $_.Name.StartsWith('.') -and $_.Name -ne $ReleaseName } |
    Remove-Item -Recurse -Force
$CurrentFile = Join-Path $InstallRoot "current.json"
$CurrentNext = Join-Path $InstallRoot ".current.$PID.json"
$currentJson = @{ versionId = $ReleaseName } | ConvertTo-Json -Compress
[IO.File]::WriteAllText($CurrentNext, "$currentJson`n", [Text.UTF8Encoding]::new($false))
Move-Item -LiteralPath $CurrentNext -Destination $CurrentFile -Force
if (-not (Test-Path -LiteralPath $OwnerFile)) {
    New-Item -ItemType Directory -Force $IdentityDir | Out-Null
    $OwnerId = "owner_$([Guid]::NewGuid().ToString('D').ToLowerInvariant())"
    $OwnerContent = @{ schemaVersion = 1; ownerId = $OwnerId } | ConvertTo-Json -Compress
    $OwnerStaging = "$OwnerFile.tmp.$PID"
    [IO.File]::WriteAllText($OwnerStaging, $OwnerContent, [Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $OwnerStaging -Destination $OwnerFile -Force
}
if (-not (Test-Path -LiteralPath $SoulFile) -and (Test-Path -LiteralPath $SoulSource -PathType Leaf)) {
    New-Item -ItemType Directory -Force $AxonUserDir | Out-Null
    Copy-Item -LiteralPath $SoulSource -Destination $SoulFile
}
$Cmd = Join-Path $InstallDir "axon.cmd"
"@echo off`r`n`"$(Join-Path $LauncherDir 'axon-launcher.exe')`" %*`r`n" | Set-Content -Encoding ascii $Cmd
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
$PathEntries = @()
if (-not [string]::IsNullOrWhiteSpace($UserPath)) {
    $PathEntries = $UserPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
}
foreach ($PathEntry in @($InstallDir, $LauncherDir)) {
    if (-not ($PathEntries | Where-Object { $_.TrimEnd('\') -ieq $PathEntry.TrimEnd('\') })) {
        $PathEntries += $PathEntry
    }
}
if ($PathEntries.Count -gt 0) {
    [Environment]::SetEnvironmentVariable("Path", ($PathEntries -join ';'), "User")
}
    $AxonLauncherPath = Join-Path $LauncherDir "axon-launcher.exe"
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        & $AxonLauncherPath daemon install
        $DaemonExitCode = $LASTEXITCODE
    } else {
        $script = "if ([Security.Principal.WindowsIdentity]::GetCurrent().User.Value -ne '$($identity.User.Value)') { exit 1 }; & '$($AxonLauncherPath.Replace("'", "''"))' daemon install; exit `$LASTEXITCODE"
        $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
        $pwsh = (Get-Command pwsh.exe -CommandType Application -ErrorAction Stop).Source
        $process = Start-Process -FilePath $pwsh -ArgumentList @("-NoProfile", "-EncodedCommand", $encoded) -Verb RunAs -WindowStyle Hidden -Wait -PassThru
        $DaemonExitCode = $process.ExitCode
    }
    if ($DaemonExitCode -ne 0) { throw "daemon install exit=$DaemonExitCode" }
Write-Host "Axon v$VersionId 已发布到 $VersionDir"
Write-Host "Windows 当前用户最高权限计划任务 AxonDaemon 已安装并启动"
Write-Host "已将 CLI 与 launcher 目录加入当前用户 PATH；请重新打开终端后运行: axon --version"
"#
}

fn unix_install_script() -> &'static str {
    r#"#!/usr/bin/env sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SOURCE="$SCRIPT_DIR/bin/axon"
LAUNCHER_SOURCE="$SCRIPT_DIR/bin/axon-launcher"
VERSION_ID="$(sed -n 's/.*\"version\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p' "$SCRIPT_DIR/axon-release-manifest.json")"
TARGET_ID="$(sed -n 's/.*\"target\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p' "$SCRIPT_DIR/axon-release-manifest.json")"
INSTALL_DIR="${1:-${AXON_INSTALL_DIR:-$HOME/.axon/bin}}"
INSTALL_ROOT="${AXON_INSTALL_ROOT:-$HOME/.axon/bin}"
AXON_USER_DIR="${HOME}/.axon"
SOUL_SOURCE="$SCRIPT_DIR/defaults/SOUL.md"
SOUL_FILE="$AXON_USER_DIR/SOUL.md"
IDENTITY_DIR="${AXON_USER_DIR}/identity"
OWNER_FILE="${IDENTITY_DIR}/owner.json"

if [ ! -f "$SOURCE" ]; then
    echo "Axon binary not found: $SOURCE" >&2
    exit 1
fi
if [ ! -f "$LAUNCHER_SOURCE" ] || [ -z "$VERSION_ID" ]; then
    echo "Axon launcher 或版本清单不存在" >&2
    exit 1
fi
if [ -z "$TARGET_ID" ]; then
    echo "package 版本清单缺少 target" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    BINARY_HASH="$(sha256sum "$SOURCE" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
    BINARY_HASH="$(shasum -a 256 "$SOURCE" | awk '{print $1}')"
else
    echo "需要 sha256sum 或 shasum 以验证 Axon 二进制" >&2
    exit 1
fi
RELEASE_NAME="${VERSION_ID}"
mkdir -p "$INSTALL_ROOT/versions" "$INSTALL_ROOT/launcher" "$INSTALL_DIR"
RELEASE_DIR="$INSTALL_ROOT/versions/$RELEASE_NAME"
STAGING_DIR="$INSTALL_ROOT/versions/.staging.${RELEASE_NAME}.$$"
if [ -x "$INSTALL_ROOT/launcher/axon-launcher" ]; then
    "$INSTALL_ROOT/launcher/axon-launcher" daemon stop >/dev/null 2>&1 || true
fi
pkill -f "$INSTALL_ROOT/versions/.*/axon-desktop" >/dev/null 2>&1 || true
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"
cp "$SOURCE" "$STAGING_DIR/axon"
chmod 755 "$STAGING_DIR/axon"
if ! "$STAGING_DIR/axon" --version >/dev/null 2>&1; then
    rm -rf "$STAGING_DIR"
    echo "staging 版本无法通过 axon --version 验证" >&2
    exit 1
fi
rm -rf "$RELEASE_DIR"
mv "$STAGING_DIR" "$RELEASE_DIR"
for release in "$INSTALL_ROOT/versions"/*; do
    [ -d "$release" ] || continue
    release_id=$(basename "$release")
    case "$release_id" in
        .*) continue ;;
    esac
    [ "$release_id" = "$RELEASE_NAME" ] || rm -rf -- "$release"
done
cp "$LAUNCHER_SOURCE" "$INSTALL_ROOT/launcher/axon-launcher"
chmod 755 "$INSTALL_ROOT/launcher/axon-launcher"
printf '{"versionId":"%s"}\n' "$RELEASE_NAME" > "$INSTALL_ROOT/.current.$$.json"
mv -f "$INSTALL_ROOT/.current.$$.json" "$INSTALL_ROOT/current.json"
if [ ! -e "$OWNER_FILE" ] && [ ! -L "$OWNER_FILE" ]; then
    mkdir -p "$IDENTITY_DIR"
    if command -v uuidgen >/dev/null 2>&1; then
        OWNER_UUID="$(uuidgen | tr '[:upper:]' '[:lower:]')"
    elif [ -r /proc/sys/kernel/random/uuid ]; then
        OWNER_UUID="$(cat /proc/sys/kernel/random/uuid)"
    else
        OWNER_HEX="$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
        OWNER_UUID="$(printf '%s' "$OWNER_HEX" | sed 's/^\(........\)\(....\)\(....\)\(....\)\(............\)$/\1-\2-\3-\4-\5/')"
    fi
    printf '{"schemaVersion":1,"ownerId":"owner_%s"}\n' "$OWNER_UUID" > "$OWNER_FILE.tmp.$$"
    chmod 600 "$OWNER_FILE.tmp.$$"
    mv -f "$OWNER_FILE.tmp.$$" "$OWNER_FILE"
fi
if [ ! -e "$SOUL_FILE" ] && [ -f "$SOUL_SOURCE" ]; then
    mkdir -p "$AXON_USER_DIR"
    cp "$SOUL_SOURCE" "$SOUL_FILE"
fi
ln -sfn "$INSTALL_ROOT/launcher/axon-launcher" "$INSTALL_DIR/axon"
PATH_PROFILE="${HOME}/.profile"
case "${SHELL:-sh}" in
    */zsh) PATH_PROFILE="${ZDOTDIR:-${HOME}}/.zshrc" ;;
    */bash) PATH_PROFILE="${HOME}/.bashrc" ;;
    */fish) PATH_PROFILE="${HOME}/.config/fish/config.fish" ;;
esac
mkdir -p "$(dirname "$PATH_PROFILE")"
case "${SHELL:-sh}" in
    */fish)
    grep -Fqx "fish_add_path \"$INSTALL_DIR\" \"$INSTALL_ROOT/launcher\"" "$PATH_PROFILE" 2>/dev/null ||
        printf '\nfish_add_path "%s" "%s"\n' "$INSTALL_DIR" "$INSTALL_ROOT/launcher" >> "$PATH_PROFILE"
        ;;
    *)
    PATH_LINE="export PATH=\"$INSTALL_DIR:$INSTALL_ROOT/launcher:\$PATH\""
    grep -Fqx "$PATH_LINE" "$PATH_PROFILE" 2>/dev/null || printf '\n%s\n' "$PATH_LINE" >> "$PATH_PROFILE"
        ;;
esac
export PATH="$INSTALL_DIR:$INSTALL_ROOT/launcher:$PATH"
"$INSTALL_ROOT/launcher/axon-launcher" daemon install
echo "Axon v$VERSION_ID 已发布到 $RELEASE_DIR"
echo "Daemon service installed"
echo "CLI 与 launcher 目录已加入 PATH；重新打开终端后运行: axon --version"
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_script_name_matches_target() {
        let root = temp_root("install-script");
        fs::create_dir_all(&root).unwrap();

        let windows = create_install_script(&root, "x86_64-pc-windows-msvc").unwrap();
        let unix = create_install_script(&root, "x86_64-unknown-linux-gnu").unwrap();

        assert_eq!(
            windows.path.file_name().and_then(|name| name.to_str()),
            Some(WINDOWS_INSTALL_SCRIPT)
        );
        assert_eq!(
            unix.path.file_name().and_then(|name| name.to_str()),
            Some(UNIX_INSTALL_SCRIPT)
        );
        assert!(!fs::read_to_string(&windows.path).unwrap().contains("setx"));
        assert!(!fs::read_to_string(&unix.path).unwrap().contains("sudo"));
        let windows_script = fs::read_to_string(&windows.path).unwrap();
        assert!(windows_script.contains("$InstallDir"));
        assert!(windows_script.contains("$LauncherDir"));
        assert!(windows_script.contains("$StagingDir"));
        assert!(windows_script.contains("$ReleaseName"));
        assert!(!windows_script.contains("$BinaryHash"));
        assert!(
            windows_script.contains("Move-Item -LiteralPath $StagingDir -Destination $VersionDir")
        );
        let unix_script = fs::read_to_string(&unix.path).unwrap();
        assert!(unix_script.contains("$INSTALL_DIR:$INSTALL_ROOT/launcher"));
        assert!(unix_script.contains("fish_add_path"));
        assert!(unix_script.contains("BINARY_HASH"));
        assert!(unix_script.contains("STAGING_DIR"));
        assert!(unix_script.contains("RELEASE_NAME"));
        assert!(!unix_script.contains(r#"RELEASE_NAME="${VERSION_ID}-"#));
        assert!(
            !fs::read_to_string(&windows.path)
                .unwrap()
                .contains("Directory]::Delete")
        );
        assert!(
            !fs::read_to_string(&unix.path)
                .unwrap()
                .contains(r#"rm -rf "$RELEASE""#)
        );

        let _ = fs::remove_dir_all(root);
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        axon_test_support::test_temp_root().join(format!("axon-package-{label}-{nonce}"))
    }
}
