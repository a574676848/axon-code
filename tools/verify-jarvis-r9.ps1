param(
    [switch]$RealDevice,
    [string]$ReportPath = ""
)

$ErrorActionPreference = "Stop"
$workspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$userAxonRoot = Join-Path $HOME ".axon"
$guardedFiles = @(
    (Join-Path $userAxonRoot "config.toml"),
    (Join-Path $userAxonRoot ".env")
)

function Get-FileSnapshot([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        return @{ Exists = $false; Hash = "" }
    }
    return @{ Exists = $true; Hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash }
}

function Assert-FileSnapshotsEqual([hashtable]$Before, [hashtable]$After, [string]$Path) {
    if ($Before.Exists -ne $After.Exists -or $Before.Hash -ne $After.Hash) {
        throw "R9 测试污染用户配置：$Path"
    }
}

$snapshotsBefore = @{}
foreach ($path in $guardedFiles) {
    $snapshotsBefore[$path] = Get-FileSnapshot $path
}

$previousRealDevice = $env:JARVIS_REAL_DEVICE
$startedAt = [DateTimeOffset]::UtcNow
try {
    Set-Location $workspaceRoot
    Write-Output "==> R9 P0 Gateway Workspace 边界测试"
    & cargo test -p axon-daemon gateway_ --lib
    if ($LASTEXITCODE -ne 0) { throw "Gateway 边界测试失败" }

    Write-Output "==> R9 P1/P2 Task 持久化与跨进程 SQLite 测试"
    & cargo test -p axon-daemon daemon_host_task_tools::tests --lib
    if ($LASTEXITCODE -ne 0) { throw "Task 工具测试失败" }
    & cargo test -p axon-daemon cross_process_ledger_append_is_serialized --lib -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw "跨进程 SQLite 测试失败" }

    if ($RealDevice) {
        $env:JARVIS_REAL_DEVICE = "1"
        Write-Output "==> R9 P2 真机 Daemon 重启验收"
        & bash tools/verify-daemon-restart.sh
        if ($LASTEXITCODE -ne 0) { throw "真机重启验收失败" }
    } else {
        Write-Output "==> 已跳过真机重启：需要显式传入 -RealDevice"
    }

    $snapshotsAfter = @{}
    foreach ($path in $guardedFiles) {
        $snapshotsAfter[$path] = Get-FileSnapshot $path
        Assert-FileSnapshotsEqual $snapshotsBefore[$path] $snapshotsAfter[$path] $path
    }

    $report = [ordered]@{
        suite = "jarvis-r9"
        status = "passed"
        realDevice = [bool]$RealDevice
        startedAt = $startedAt.ToString("o")
        finishedAt = [DateTimeOffset]::UtcNow.ToString("o")
        guardedFiles = $guardedFiles
        productionConfigUnchanged = $true
    } | ConvertTo-Json -Compress
    Write-Output $report
    if ($ReportPath) {
        Set-Content -LiteralPath $ReportPath -Value $report -Encoding utf8
    }
} finally {
    if ($null -eq $previousRealDevice) {
        Remove-Item Env:JARVIS_REAL_DEVICE -ErrorAction SilentlyContinue
    } else {
        $env:JARVIS_REAL_DEVICE = $previousRealDevice
    }
}
