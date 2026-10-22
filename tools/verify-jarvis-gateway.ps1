param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
    [switch]$SkipCapacity,
    [switch]$SkipSoak,
    [int]$Iterations = 0,
    [int]$Cycles = 0,
    [string]$ReportPath = ""
)

$ErrorActionPreference = "Stop"
Set-Location $WorkspaceRoot

$reports = New-Object System.Collections.Generic.List[string]

function Invoke-GatewayTests {
    Write-Output "==> R8-GW-07 Gateway 边界回归：生命周期、认证、连接上限、mailbox、cursor、HTTP、WS"
    $output = & cargo test --release -p axon-daemon gateway --lib -- --nocapture 2>&1
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { $_ }
    if ($exitCode -ne 0) {
        Write-Error "Gateway 边界回归失败，退出码 $exitCode"
        exit 1
    }
    $reports.Add((ConvertTo-Json -Compress @{ case = "gateway_boundary"; status = "passed" }))
}

function Invoke-CapacityAudit {
    if ($SkipCapacity) {
        return
    }
    Write-Output "==> R8-GW-07 容量审计：1k/10k backlog 与 8 线程并发入口"
    $output = & pwsh -NoProfile -File (Join-Path $PSScriptRoot "verify-jarvis-capacity.ps1") -WorkspaceRoot $WorkspaceRoot 2>&1
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { $_ }
    if ($exitCode -ne 0) {
        Write-Error "Gateway 容量审计失败，退出码 $exitCode"
        exit 1
    }
    foreach ($line in $output) {
        if ($line -match '(\{.+\})\s*$') {
            $reports.Add([string]$Matches[1])
        }
    }
}

function Invoke-SoakAudit {
    if ($SkipSoak) {
        return
    }
    Write-Output "==> R8-GW-07 常驻恢复审计：Host lease、Execution recovery、Delivery retry"
    $arguments = @(
        "-NoProfile",
        "-File", (Join-Path $PSScriptRoot "verify-jarvis-soak.ps1"),
        "-WorkspaceRoot", $WorkspaceRoot
    )
    if ($Iterations -gt 0) {
        $arguments += @("-Iterations", $Iterations)
    }
    if ($Cycles -gt 0) {
        $arguments += @("-Cycles", $Cycles)
    }
    $output = & pwsh @arguments 2>&1
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { $_ }
    if ($exitCode -ne 0) {
        Write-Error "Gateway 常驻恢复审计失败，退出码 $exitCode"
        exit 1
    }
    foreach ($line in $output) {
        if ($line -match '(\{.+\})\s*$') {
            $reports.Add([string]$Matches[1])
        }
    }
}

Invoke-GatewayTests
Invoke-CapacityAudit
Invoke-SoakAudit

if ($ReportPath) {
    $reports | Set-Content -LiteralPath $ReportPath -Encoding utf8
    Write-Output "Gateway 验收报告已写入 $ReportPath"
}
Write-Output "Gateway 验收报告条数: $($reports.Count)"
Write-Output "说明：Feishu、外部 HTTP 和外部 WS 的真实网络回执必须在部署现场验证；本脚本不以 Mock 回执替代真机验收。"
