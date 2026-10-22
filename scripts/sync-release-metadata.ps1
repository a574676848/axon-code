param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [Parameter(Mandatory = $true)]
    [string]$DistDir
)

$ErrorActionPreference = "Stop"

function Get-Sha256Hex {
    param([string]$Path)
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLower()
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$distRoot = Resolve-Path -LiteralPath $DistDir
$normalizedVersion = $Version.TrimStart("v")
$workspaceCargo = Join-Path $repoRoot "Cargo.toml"
$workspaceCargoContent = Get-Content -LiteralPath $workspaceCargo -Raw
$workspaceVersionMatch = [regex]::Match($workspaceCargoContent, '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
if (-not $workspaceVersionMatch.Success) {
    throw "无法从 Cargo.toml 的 [workspace.package] 读取版本号"
}
$workspaceVersion = $workspaceVersionMatch.Groups[1].Value
if ($normalizedVersion -ne $workspaceVersion) {
    throw "传入版本 $normalizedVersion 与 workspace 版本 $workspaceVersion 不一致"
}
$wingetManifestDir = Join-Path $repoRoot "installer\winget\manifests\a\AlexZ\Axon\$normalizedVersion"

$windowsZip = Join-Path $distRoot "axon-v$normalizedVersion-x86_64-pc-windows-msvc.zip"
$windowsMsi = Join-Path $distRoot "axon-v$normalizedVersion-x86_64-pc-windows-msvc.msi"
$macArmTar = Join-Path $distRoot "axon-v$normalizedVersion-aarch64-apple-darwin.tar.gz"
$macX64Tar = Join-Path $distRoot "axon-v$normalizedVersion-x86_64-apple-darwin.tar.gz"
$linuxX64Tar = Join-Path $distRoot "axon-v$normalizedVersion-x86_64-unknown-linux-gnu.tar.gz"
$linuxArmTar = Join-Path $distRoot "axon-v$normalizedVersion-aarch64-unknown-linux-gnu.tar.gz"

$requiredArtifacts = @(
    $windowsZip,
    $windowsMsi,
    $macArmTar,
    $macX64Tar,
    $linuxX64Tar,
    $linuxArmTar
)

foreach ($artifact in $requiredArtifacts) {
    if (-not (Test-Path -LiteralPath $artifact)) {
        throw "缺少 release 产物：$artifact"
    }
}

$homebrewPath = Join-Path $repoRoot "homebrew\axon.rb"
$scoopPath = Join-Path $repoRoot "installer\scoop\axon.json"
$wingetInstallerPath = Join-Path $wingetManifestDir "AlexZ.Axon.installer.yaml"

$homebrewContent = Get-Content -LiteralPath $homebrewPath -Raw
$homebrewContent = $homebrewContent.Replace("__VERSION__", $normalizedVersion)
$homebrewContent = $homebrewContent.Replace("__MACOS_ARM64_SHA256__", (Get-Sha256Hex $macArmTar))
$homebrewContent = $homebrewContent.Replace("__MACOS_X64_SHA256__", (Get-Sha256Hex $macX64Tar))
$homebrewContent = $homebrewContent.Replace("__LINUX_X64_SHA256__", (Get-Sha256Hex $linuxX64Tar))
$homebrewContent = $homebrewContent.Replace("__LINUX_ARM64_SHA256__", (Get-Sha256Hex $linuxArmTar))
Set-Content -LiteralPath $homebrewPath -Value $homebrewContent -Encoding utf8NoBOM

$scoop = Get-Content -LiteralPath $scoopPath -Raw | ConvertFrom-Json
$scoop.version = $normalizedVersion
$scoop.url = "https://github.com/a574676848/axon-code/releases/download/v$normalizedVersion/axon-v$normalizedVersion-x86_64-pc-windows-msvc.zip"
$scoop.extract_dir = "axon-v$normalizedVersion-x86_64-pc-windows-msvc"
$scoop.hash = Get-Sha256Hex $windowsZip
$scoop.autoupdate.url = $scoop.url.Replace($normalizedVersion, '$version')
$scoopJson = $scoop | ConvertTo-Json -Depth 8
Set-Content -LiteralPath $scoopPath -Value $scoopJson -Encoding utf8NoBOM

$wingetLocalePath = Join-Path $wingetManifestDir "AlexZ.Axon.locale.zh-CN.yaml"
$wingetVersionPath = Join-Path $wingetManifestDir "AlexZ.Axon.yaml"
if ((Test-Path -LiteralPath $wingetInstallerPath) -and
    (Test-Path -LiteralPath $wingetLocalePath) -and
    (Test-Path -LiteralPath $wingetVersionPath)) {
    $wingetContent = Get-Content -LiteralPath $wingetInstallerPath -Raw
    $wingetContent = $wingetContent.Replace("__VERSION__", $normalizedVersion)
    $wingetContent = $wingetContent.Replace("__WINDOWS_MSI_SHA256__", (Get-Sha256Hex $windowsMsi))
    Set-Content -LiteralPath $wingetInstallerPath -Value $wingetContent -Encoding utf8NoBOM

    $wingetLocale = (Get-Content -LiteralPath $wingetLocalePath -Raw).Replace("__VERSION__", $normalizedVersion)
    Set-Content -LiteralPath $wingetLocalePath -Value $wingetLocale -Encoding utf8NoBOM
    $wingetVersion = (Get-Content -LiteralPath $wingetVersionPath -Raw).Replace("__VERSION__", $normalizedVersion)
    Set-Content -LiteralPath $wingetVersionPath -Value $wingetVersion -Encoding utf8NoBOM
}
