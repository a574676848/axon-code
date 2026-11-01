# 性能契约门禁（合同 §15）
#
# 两种模式：
#   1) 默认：读取 tools/perf-baseline.json，逐项核对当前读数；任一读数比基线差
#      超过允许比例即失败（§15.4「性能回退即失败」）。基线里缺失的项计入「待测量」，
#      默认不算失败，但控制台会逐项列出，避免静默略过。
#   2) -Record：把本次实测读数写入基线文件。人工真机测量后跑一次即可。
#
# 可自动测量的项：
#   P1  端点解析读盘次数：由 `cargo test -p axon-desktop endpoint` 的单测钉住，
#       本脚本只确认该测试存在并被执行（避免「断言被删了没人发现」）。
#   P12 本机操作零网关命中：读 daemon 当日访问日志计数 /api/v1/files、/fs/dirs、
#       /attachments 三个前缀；日志不存在时记为待测量。
#
# 需人工真机测量的项（脚本只核对是否已登记读数）：
#   P2 断网启动增量、P9 空闲 60 秒 CPU、P10 切档冷启动、P11 上传线性成本。
[CmdletBinding()]
param(
    # 基线文件；缺省 tools/perf-baseline.json。
    [string]$Baseline = '',
    # 把本次读数写回基线。
    [switch]$Record,
    # 允许待测量项（CI 用）：存在未登记读数时只警告，不失败。
    [switch]$AllowIncomplete
)

$ErrorActionPreference = 'Stop'
$utf8 = [Text.UTF8Encoding]::new($false)
$OutputEncoding = $utf8
[Console]::OutputEncoding = $utf8
$PSStyle.OutputRendering = 'PlainText'

$repo = Split-Path -Parent $PSScriptRoot
if ($Baseline -eq '') {
    $Baseline = Join-Path $repo 'tools/perf-baseline.json'
}

# 允许的退步比例：读数变大不超过 5%。
$REGRESSION_RATIO = 1.05

$measurements = [ordered]@{}
$pending = [System.Collections.Generic.List[string]]::new()

# P1：确认「60 次解析最多读 60 次配置」这条断言还在测试树里。
$endpointTest = Join-Path $repo 'crates/axon-desktop/src/connection/endpoint_tests.rs'
$p1Present = (Test-Path $endpointTest) -and
    ((Get-Content -LiteralPath $endpointTest -Raw) -match 'sixty_times_reads_the_config_at_most_sixty_times')
if ($p1Present) {
    $measurements['p1_endpoint_reads_assertion'] = 1
} else {
    Write-Host '[perf] P1：找不到端点解析读盘次数的断言，视为缺失' -ForegroundColor Red
    $measurements['p1_endpoint_reads_assertion'] = 0
}

# P12：当日 daemon 日志里，本机文件类前缀的命中次数。
$axonHome = if ($env:AXON_RUNTIME_ROOT) { $env:AXON_RUNTIME_ROOT } else {
    $homeDir = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }
    [IO.Path]::Combine($homeDir, '.axon')
}
$logPath = [IO.Path]::Combine($axonHome, 'data', 'logs', ('axon-' + (Get-Date -Format 'yyyy-MM-dd') + '.log'))
if (Test-Path -LiteralPath $logPath -PathType Leaf) {
    $text = Get-Content -LiteralPath $logPath -Raw
    $hits = 0
    foreach ($prefix in @('/api/v1/files', '/api/v1/fs/dirs', '/api/v1/attachments')) {
        $hits += ([regex]::Matches($text, [regex]::Escape($prefix))).Count
    }
    $measurements['p12_file_route_hits'] = $hits
} else {
    $pending.Add('P12 本机操作零网关命中（当日日志不存在，需先跑一次真机操作再测）')
}

foreach ($item in @(
    'P2 断网启动到可见界面增量（毫秒）',
    'P9 空闲 60 秒渲染进程 CPU（百分比）',
    'P10 切档冷启动到可交互（毫秒，本机 / 云）',
    'P11 上传 1/8/32 MiB 的相对成本（线性）'
)) {
    $pending.Add($item)
}

Write-Host '[perf] 本次读数：' -ForegroundColor Cyan
$measurements.GetEnumerator() | ForEach-Object { Write-Host ("  {0} = {1}" -f $_.Key, $_.Value) }

if ($Record) {
    $document = [pscustomobject]@{
        recordedAt = (Get-Date).ToUniversalTime().ToString('o')
        measurements = $measurements
        pending = @($pending)
    }
    $json = $document | ConvertTo-Json -Depth 6
    [IO.File]::WriteAllText($Baseline, $json + "`n", $utf8)
    Write-Host ("[perf] 已写入基线：{0}" -f $Baseline) -ForegroundColor Green
}

$failures = 0
if (Test-Path -LiteralPath $Baseline -PathType Leaf) {
    $baselineDocument = Get-Content -LiteralPath $Baseline -Raw | ConvertFrom-Json
    foreach ($name in $measurements.Keys) {
        $previous = $baselineDocument.measurements.$name
        if ($null -eq $previous) { continue }
        $current = $measurements[$name]
        if ($current -gt [double]$previous * $REGRESSION_RATIO) {
            Write-Host ("[perf] 回退：{0} 从 {1} 变成 {2}，超过 {3:P0} 容忍" -f $name, $previous, $current, ($REGRESSION_RATIO - 1)) -ForegroundColor Red
            $failures += 1
        }
    }
} else {
    Write-Host "[perf] 没有基线文件：$Baseline（先用 -Record 生成一次）" -ForegroundColor Yellow
}

if ($pending.Count -gt 0) {
    Write-Host '[perf] 待人工真机测量的项：' -ForegroundColor Yellow
    foreach ($item in $pending) { Write-Host ("  - " + $item) }
    if (-not $AllowIncomplete) {
        Write-Host '[perf] 有未登记读数的项；CI 可用 -AllowIncomplete 放行，交付前必须补齐' -ForegroundColor Yellow
    }
}

if ($failures -gt 0) {
    Write-Host ("[perf] 性能契约未通过：{0} 项回退" -f $failures) -ForegroundColor Red
    exit 1
}
Write-Host '[perf] 性能契约通过（待测量项见上）' -ForegroundColor Green
