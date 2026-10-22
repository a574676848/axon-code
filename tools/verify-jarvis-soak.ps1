param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
    [int]$Iterations = 0,
    [int]$Cycles = 0,
    [string]$ReportPath = ""
)

$ErrorActionPreference = "Stop"
Set-Location $WorkspaceRoot

if ($Iterations -gt 0) {
    $env:JARVIS_SOAK_ITERATIONS = [string]$Iterations
}
if ($Cycles -gt 0) {
    $env:JARVIS_DELIVERY_SOAK_CYCLES = [string]$Cycles
}

$reports = New-Object System.Collections.Generic.List[string]

function Invoke-SoakReport {
    param(
        [string]$Label,
        [string]$Package,
        [string]$Filter
    )

    Write-Output "==> $Label"
    $output = & cargo test --release -p $Package --lib $Filter -- --ignored --nocapture 2>&1
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { $_ }
    if ($exitCode -ne 0) {
        Write-Error "$Label 失败，退出码 $exitCode"
        exit 1
    }
    if ($output -match 'running 0 tests') {
        Write-Error "$Label 未匹配到任何测试，验收筛选器可能已失效"
        exit 1
    }
    foreach ($line in $output) {
        if ($line -match '(\{.+\})\s*$') {
            $reports.Add([string]$Matches[1])
        }
    }
}

Invoke-SoakReport -Label "JARVIS 常驻 soak（R7-07）：Host heartbeat、lease 续租与 Execution churn" -Package "axon-daemon" -Filter "daemon_host_soak_tests"
Invoke-SoakReport -Label "JARVIS 投递 soak（R7-07）：outbox 退避重试与崩溃恢复" -Package "axon-remote-runtime" -Filter "remote_delivery_soak_tests"

if ($ReportPath) {
    $reports | Set-Content -LiteralPath $ReportPath -Encoding utf8
    Write-Output "soak 报告已写入 $ReportPath"
}
Write-Output "soak 报告条数: $($reports.Count)"
