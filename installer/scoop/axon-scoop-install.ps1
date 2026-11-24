param(
    [Parameter(Mandatory = $true)]
    [string]$PackageRoot,
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [Parameter(Mandatory = $true)]
    [string]$Target
)

$ErrorActionPreference = "Stop"
$Source = Join-Path $PackageRoot "axon.exe"
$LauncherSource = Join-Path $PackageRoot "launcher\axon-launcher.exe"
$InstallRoot = Join-Path $HOME ".axon\bin"
$VersionsDir = Join-Path $InstallRoot "versions"
$LauncherDir = Join-Path $InstallRoot "launcher"
$LauncherPath = Join-Path $LauncherDir "axon-launcher.exe"
$UserDir = Join-Path $HOME ".axon"
$SoulSource = Join-Path $PackageRoot "assets\defaults\SOUL.md"
$SoulFile = Join-Path $UserDir "SOUL.md"

if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
    throw "Scoop package 缺少 Axon 二进制：$Source"
}
if (-not (Test-Path -LiteralPath $LauncherSource -PathType Leaf)) {
    throw "Scoop package 缺少稳定 launcher：$LauncherSource"
}

$ReleaseName = $Version
$ReleaseDir = Join-Path $VersionsDir $ReleaseName
$StagingDir = Join-Path $VersionsDir ".staging.$ReleaseName.$PID"
New-Item -ItemType Directory -Force -Path $VersionsDir, $LauncherDir | Out-Null
if (Test-Path -LiteralPath $LauncherPath -PathType Leaf) {
    & $LauncherPath daemon stop *> $null
}
Get-Process axon-desktop -ErrorAction SilentlyContinue | Where-Object {
    $_.Path -and $_.Path.StartsWith("$InstallRoot\", [StringComparison]::OrdinalIgnoreCase)
} | Stop-Process -Force
Remove-Item -LiteralPath $StagingDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $StagingDir | Out-Null
$StagedSource = Join-Path $StagingDir "axon.exe"
Copy-Item -LiteralPath $Source -Destination $StagedSource
& $StagedSource --version *> $null
if ($LASTEXITCODE -ne 0) {
    Remove-Item -LiteralPath $StagingDir -Recurse -Force -ErrorAction SilentlyContinue
    throw "staging 版本无法通过 axon --version 验证"
}
if (Test-Path -LiteralPath $ReleaseDir -PathType Container) {
    Remove-Item -LiteralPath $ReleaseDir -Recurse -Force
}
Move-Item -LiteralPath $StagingDir -Destination $ReleaseDir
Get-ChildItem -LiteralPath $VersionsDir -Directory -ErrorAction SilentlyContinue |
    Where-Object { -not $_.Name.StartsWith('.') -and $_.Name -ne $ReleaseName } |
    Remove-Item -Recurse -Force

$LauncherPath = Join-Path $LauncherDir "axon-launcher.exe"
Copy-Item -LiteralPath $LauncherSource -Destination $LauncherPath -Force
$CurrentFile = Join-Path $InstallRoot "current.json"
$CurrentNext = Join-Path $InstallRoot ".current.$PID.json"
$CurrentJson = @{ versionId = $ReleaseName } | ConvertTo-Json -Compress
[IO.File]::WriteAllText($CurrentNext, "$CurrentJson`n", [Text.UTF8Encoding]::new($false))
Move-Item -LiteralPath $CurrentNext -Destination $CurrentFile -Force

if (-not (Test-Path -LiteralPath $SoulFile -PathType Leaf) -and
    (Test-Path -LiteralPath $SoulSource -PathType Leaf)) {
    New-Item -ItemType Directory -Force $UserDir | Out-Null
    Copy-Item -LiteralPath $SoulSource -Destination $SoulFile
}

& $LauncherPath daemon install
if ($LASTEXITCODE -ne 0) {
    throw "Daemon 安装/重启失败，exit=$LASTEXITCODE"
}
Write-Host "Axon v$Version 已发布到 $ReleaseDir"
