# Axon Windows PowerShell 源码安装脚本
# 源码安装与发行版共用 $HOME\.axon\bin 版本树，稳定 PATH 入口就是该 bin 目录。
[CmdletBinding()]
param([string]$ExpectedUserSid = "", [switch]$BinariesOnly)
$ErrorActionPreference = "Stop"
$utf8 = [Text.UTF8Encoding]::new($false)
$OutputEncoding = $utf8
[Console]::OutputEncoding = $utf8
[Console]::InputEncoding = $utf8
$PSStyle.OutputRendering = "PlainText"
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if ($ExpectedUserSid -and $identity.User.Value -ne $ExpectedUserSid) { throw "提权账号与原登录用户不同，已停止安装。" }
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $BinariesOnly -and -not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    $logPath = Join-Path ([IO.Path]::GetTempPath()) "axon-source-install-$PID.log"
    $scriptPath = $PSCommandPath.Replace("'", "''")
    $bootstrap = "Start-Transcript -LiteralPath '$($logPath.Replace("'", "''"))' -Force; try { & '$scriptPath' -ExpectedUserSid '$($identity.User.Value)'; if (-not `$?) { exit 1 } } catch { Write-Output `$_; exit 1 } finally { Stop-Transcript }"
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($bootstrap))
    Write-Host "[axon] 需要 UAC 授权，安装日志：$logPath" -ForegroundColor Cyan
    $process = Start-Process -FilePath (Join-Path $PSHOME "pwsh.exe") -ArgumentList @("-NoProfile", "-EncodedCommand", $encoded) -Verb RunAs -WindowStyle Hidden -Wait -PassThru
    if (Test-Path $logPath) { Get-Content $logPath }
    if ($process.ExitCode -ne 0) { throw "源码安装失败，退出码：$($process.ExitCode)" }
    return
}
$root = Split-Path -Parent $PSScriptRoot
$cargo = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
if ($null -eq $cargo) {
    $cargoPath = Join-Path $HOME ".cargo\bin\cargo.exe"
    if (Test-Path $cargoPath) { $cargo = [pscustomobject]@{ Source = $cargoPath } }
}
if ($null -eq $cargo) { throw "未找到 cargo，请先安装 Rust 工具链。" }
$installRoot = if ($env:AXON_INSTALL_ROOT) { $env:AXON_INSTALL_ROOT } else { Join-Path $HOME ".axon\bin" }
$installDir = if ($env:AXON_INSTALL_DIR) { $env:AXON_INSTALL_DIR } else { $installRoot }
$launcherDir = Join-Path $installRoot "launcher"
$versionsDir = Join-Path $installRoot "versions"
$currentFile = Join-Path $installRoot "current.json"
$launcherPath = Join-Path $launcherDir "axon-launcher.exe"
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME ".cargo" }
$cargoAxon = Join-Path $cargoHome "bin\axon.exe"
$stagingRoot = Join-Path ([IO.Path]::GetTempPath()) "axon-source-install-$PID"
$daemonStopped = $false
$taskDisabled = $false
$previousLauncher = Test-Path $launcherPath

function Wait-DaemonStopped([string]$path) {
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    while ([DateTime]::UtcNow -lt $deadline) {
        $rawStatus = (& $path daemon status 2>$null | Out-String).Trim()
        $status = $null
        if ($rawStatus) {
            try { $status = $rawStatus | ConvertFrom-Json -ErrorAction Stop } catch { $status = $null }
        }
        if (($null -ne $status -and -not $status.running) -or
            ($null -eq $status -and -not (Get-Process axon -ErrorAction SilentlyContinue))) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Set-AxonPath([string]$directory) {
    $directory = $directory.TrimEnd('\')
    $launcher = (Join-Path $installRoot "launcher").TrimEnd('\')
    $entries = [System.Collections.Generic.List[string]]::new()
    foreach ($entry in (([Environment]::GetEnvironmentVariable("PATH", "User") ?? "") -split ";")) {
        $item = $entry.TrimEnd('\')
        if ($item -and $item -ine $launcher -and -not ($entries | Where-Object { $_ -ieq $item })) { $entries.Add($item) }
    }
    $null = $entries.RemoveAll([Predicate[string]]{ param($item) $item -ieq $directory })
    $entries.Insert(0, $directory)
    [Environment]::SetEnvironmentVariable("PATH", ($entries -join ";"), "User")
    $env:Path = "$directory;$(($env:Path -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $launcher -and $_.TrimEnd('\') -ine $directory } | Select-Object -Unique) -join ';')"
}

try {
    Remove-Item $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stagingRoot | Out-Null
    if ($previousLauncher) {
        & schtasks.exe /change /tn AxonDaemon /disable 2>$null | Out-Null
        $taskDisabled = $true
        & $launcherPath daemon stop 2>$null | Out-Host
        if (-not (Wait-DaemonStopped $launcherPath)) { throw "Daemon 未能在 60 秒内停止" }
        $daemonStopped = $true
    }
    # 编译产物在 Windows 上不能覆盖正在运行的映像，只停止本仓库的调试进程。
    $releaseBin = Join-Path $root "target\release"
    Get-Process axon, axon-desktop -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -and ([IO.Path]::GetDirectoryName($_.Path) -ieq $releaseBin)
    } | Stop-Process -Force
    Get-Process axon-desktop -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -and $_.Path.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)
    } | Stop-Process -Force
    Push-Location (Join-Path $root "crates\axon-desktop\ui")
    try {
        & npm ci
        if ($LASTEXITCODE -ne 0) { throw "前端依赖安装失败" }
        & npm run build
        if ($LASTEXITCODE -ne 0) { throw "前端构建失败" }
    } finally { Pop-Location }
    Write-Host "[axon] 正在编译 CLI、桌面端与稳定 launcher。" -ForegroundColor Cyan
    Push-Location $root
    try {
        & $cargo.Source build --release --locked -p axon-cli -p axon-desktop -p axon-launcher
        if ($LASTEXITCODE -ne 0) { throw "cargo build 失败，退出码：$LASTEXITCODE" }
    } finally { Pop-Location }
    $cli = Join-Path $releaseBin "axon.exe"
    $desktop = Join-Path $releaseBin "axon-desktop.exe"
    $builtLauncher = Join-Path $releaseBin "axon-launcher.exe"
    foreach ($path in @($cli, $desktop, $builtLauncher)) { if (-not (Test-Path $path)) { throw "编译完成但缺少二进制：$path" } }
    & $cli --version *> $null; if ($LASTEXITCODE -ne 0) { throw "CLI 自检失败" }
    & $desktop --self-check *> $null; if ($LASTEXITCODE -ne 0) { throw "桌面端自检失败" }
    $version = (& $cli --version).ToString().Trim().Split()[-1]
    $releaseId = $version
    $releaseDir = Join-Path $versionsDir $releaseId
    $stagedRelease = Join-Path $versionsDir ".staging.$PID"
    # 构建耗时数分钟，期间桌面端可能被重新拉起并锁住旧版本目录：替换前再停一次，
    # 并以有界重试等待映像锁释放，避免安装半途失败留下 staging 残留。
    Get-ChildItem -LiteralPath $versionsDir -Directory -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name.StartsWith('.staging.') } |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stagedRelease, $launcherDir, $installDir | Out-Null
    Copy-Item $cli (Join-Path $stagedRelease "axon.exe")
    Copy-Item $desktop (Join-Path $stagedRelease "axon-desktop.exe")
    $swapDeadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($true) {
        $swapBlocked = $false
        try {
            if (Test-Path $releaseDir) { Remove-Item $releaseDir -Recurse -Force -ErrorAction Stop }
        } catch {
            $swapBlocked = $true
            Write-Host "[axon] 旧版本目录仍被占用，正在停止占用进程后重试：$($_.Exception.Message)" -ForegroundColor Yellow
        }
        if (-not $swapBlocked) { break }
        if ([DateTime]::UtcNow -gt $swapDeadline) { throw "旧版本目录 30 秒内仍被占用，无法完成替换：$releaseDir" }
        Get-Process axon-desktop -ErrorAction SilentlyContinue | Where-Object {
            $_.Path -and $_.Path.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)
        } | Stop-Process -Force
        Start-Sleep -Milliseconds 500
    }
    Move-Item $stagedRelease $releaseDir
    Get-ChildItem -LiteralPath $versionsDir -Directory -ErrorAction SilentlyContinue |
        Where-Object { -not $_.Name.StartsWith('.') -and $_.Name -ne $releaseId } |
        Remove-Item -Recurse -Force
    Copy-Item $builtLauncher "$launcherPath.tmp.$PID" -Force
    Move-Item "$launcherPath.tmp.$PID" $launcherPath -Force
    $currentTmp = Join-Path $installRoot ".current.$PID.json"
    [IO.File]::WriteAllText($currentTmp, (@{ versionId = $releaseId } | ConvertTo-Json -Compress) + "`n", $utf8)
    Move-Item $currentTmp $currentFile -Force
    Remove-Item $cargoAxon -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path $cargoHome "bin\axon-desktop.exe") -Force -ErrorAction SilentlyContinue
    Set-AxonPath $installDir
    [IO.File]::WriteAllText((Join-Path $installDir "axon.cmd"), "@echo off`r`n`"$launcherPath`" %*`r`n", [Text.UTF8Encoding]::new($false))
    $configPath = Join-Path $HOME ".axon\config.toml"
    if (-not (Test-Path $configPath -PathType Leaf) -or (Get-Item $configPath).Length -eq 0) {
        & $launcherPath setup --provider-name openai --model gpt-4o --non-interactive --no-validate | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Axon 默认配置初始化失败，退出码：$LASTEXITCODE" }
    }
    if (-not $BinariesOnly) { & $launcherPath daemon install | Out-Host; if ($LASTEXITCODE -ne 0) { throw "Daemon 安装失败，退出码：$LASTEXITCODE" } }
    & $launcherPath daemon start | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Daemon 启动失败，退出码：$LASTEXITCODE" }
    $status = & $launcherPath daemon status | Out-String | ConvertFrom-Json
    if (-not $status.running) { throw "Daemon 未正常运行，请检查已注册服务和 data/logs 日志。" }
    Write-Host "[axon] 源码安装完成：$releaseDir" -ForegroundColor Green
} catch {
    if ($taskDisabled) { & schtasks.exe /change /tn AxonDaemon /enable 2>$null | Out-Null }
    if ($daemonStopped -and (Test-Path $launcherPath)) { & $launcherPath daemon start 2>$null | Out-Host }
    throw
} finally { Remove-Item $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue }
