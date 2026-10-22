param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
    [string]$Backlogs = $env:JARVIS_CAPACITY_BACKLOGS,
    [string]$ReportPath = ""
)

$ErrorActionPreference = "Stop"
Set-Location $WorkspaceRoot

if ($Backlogs) {
    $env:JARVIS_CAPACITY_BACKLOGS = $Backlogs
}

Write-Output "==> JARVIS R9-P1 容量审计：真实 SQLite 事实源 + DaemonHost 领取/结算"
$output = & cargo test --release -p axon-daemon --lib daemon_host_capacity_tests -- --ignored --nocapture 2>&1
$exitCode = $LASTEXITCODE
$output | ForEach-Object { $_ }
if ($exitCode -ne 0) {
    Write-Error "容量审计失败，退出码 $exitCode"
    exit 1
}

$reports = @($output | ForEach-Object { if ($_ -match '(\{.+\})\s*$') { $Matches[1] } })
if ($reports.Count -eq 0) {
    Write-Error "容量审计未输出 JSON 报告"
    exit 1
}
if ($ReportPath) {
    $reports | Set-Content -LiteralPath $ReportPath -Encoding utf8
    Write-Output "容量报告已写入 $ReportPath"
}
Write-Output "容量报告条数: $($reports.Count)（P50/P95、常驻内存、WAL 体积、最老积压时长与并发入口失败数见上方 JSON）"
