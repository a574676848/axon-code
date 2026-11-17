# 长跑观测：按固定间隔采样运行态，用于 B7（24 小时长跑）取证。
#
# 判据只看可复核的读数与单调性，不看退出码：
#   - daemon 全程存活，日志里 database is locked 与 Fenced 计数为 0；
#   - 事件面与控制面库体量在 24 小时内不出现无界增长（增量与轮次同阶，且不随时间加速）；
#   - 进程工作集不持续爬升。
#
# 用法（隔离根，不触碰真实 $HOME/.axon）：
#   pwsh -File tools/verify-soak.ps1 -RuntimeRoot $env:TEMP/axon-verify-rt -Hours 24

param(
    [Parameter(Mandatory = $true)][string]$RuntimeRoot,
    [int]$Hours = 24,
    [int]$IntervalMinutes = 5,
    [string]$OutFile = '',
    [string]$DaemonPidFile = ''
)

$ErrorActionPreference = 'Stop'
$deadline = (Get-Date).AddHours($Hours)
if ([string]::IsNullOrWhiteSpace($OutFile)) {
    $OutFile = Join-Path $RuntimeRoot 'soak-samples.jsonl'
}
if ([string]::IsNullOrWhiteSpace($DaemonPidFile)) {
    # pid 文件在运行态数据目录内，不是 Axon 根目录下。
    $DaemonPidFile = Join-Path $RuntimeRoot 'data/daemon.pid'
}

function Get-DirBytes {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) { return 0 }
    return [int64]$sum
}

function Get-FileBytes {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    return [int64](Get-Item -LiteralPath $Path).Length
}

function Get-LogErrorCounts {
    param([string]$DataDir)
    $today = Join-Path $DataDir ('logs/axon-{0}.log' -f (Get-Date -Format 'yyyy-MM-dd'))
    $result = @{ locked = 0; fenced = 0 }
    if (-not (Test-Path -LiteralPath $today)) { return $result }
    # 只数当日日志：长跑判定关心的是「本次观测窗口内新增的错误」，不是历史累积。
    $result.locked = (Select-String -LiteralPath $today -Pattern 'database is locked' -SimpleMatch -ErrorAction SilentlyContinue |
        Measure-Object).Count
    $result.fenced = (Select-String -LiteralPath $today -Pattern 'Fenced' -SimpleMatch -ErrorAction SilentlyContinue |
        Measure-Object).Count
    return $result
}

$dataDir = Join-Path $RuntimeRoot 'data'
# 观测起点基线：日志是当日累积的，包含本次长跑之前的记录。
# 只有「本次窗口内新增」的错误数才是长跑结论的依据，否则历史噪声会直接判失败。
$baseline = Get-LogErrorCounts -DataDir $dataDir
$samples = New-Object System.Collections.Generic.List[object]
Start-Sleep -Seconds 1
while ((Get-Date) -lt $deadline) {
    $running = $false
    $workingSetMb = 0.0
    if (Test-Path -LiteralPath $DaemonPidFile) {
        $rawPid = (Get-Content -LiteralPath $DaemonPidFile -Raw -ErrorAction SilentlyContinue)
        $parsed = 0
        if ([int]::TryParse(($rawPid -as [string]).Trim(), [ref]$parsed) -and $parsed -gt 0) {
            $proc = Get-Process -Id $parsed -ErrorAction SilentlyContinue
            if ($null -ne $proc) {
                $running = $true
                $workingSetMb = [math]::Round($proc.WorkingSet64 / 1MB, 2)
            }
        }
    }
    $counts = Get-LogErrorCounts -DataDir $dataDir
    $sample = [ordered]@{
        at = (Get-Date).ToString('o')
        daemon_running = $running
        working_set_mb = $workingSetMb
        data_bytes = (Get-DirBytes -Path $dataDir)
        control_bytes = (Get-FileBytes -Path (Join-Path $dataDir 'control.sqlite3'))
        control_wal_bytes = (Get-FileBytes -Path (Join-Path $dataDir 'control.sqlite3-wal'))
        outbox_bytes = (Get-FileBytes -Path (Join-Path $dataDir 'outbox.sqlite3'))
        events_bytes = (Get-DirBytes -Path (Join-Path $dataDir 'events'))
        sessions_bytes = (Get-DirBytes -Path (Join-Path $dataDir 'sessions'))
        logs_bytes = (Get-DirBytes -Path (Join-Path $dataDir 'logs'))
        memory_bytes = (Get-DirBytes -Path (Join-Path $dataDir 'memory'))
        locked_hits = ($counts.locked - $baseline.locked)
        fenced_hits = ($counts.fenced - $baseline.fenced)
    }
    $samples.Add([pscustomobject]$sample)
    ($sample | ConvertTo-Json -Compress) | Add-Content -LiteralPath $OutFile -Encoding utf8
    Start-Sleep -Seconds ($IntervalMinutes * 60)
}

$first = $samples[0]
if ($null -eq $first) {
    # 观测窗口不足一个采样点时不算通过：没有读数的长跑结论等同于没跑。
    [ordered]@{ window_hours = 0; samples = 0; verdict = 'no-samples'; evidence = $OutFile } | ConvertTo-Json
    exit 1
}
$last = $samples[$samples.Count - 1]
$hours = [math]::Max(0.01, ((Get-Date $last.at) - (Get-Date $first.at)).TotalHours)
$report = [ordered]@{
    window_hours = [math]::Round($hours, 2)
    samples = $samples.Count
    daemon_always_running = -not ($samples | Where-Object { -not $_.daemon_running })
    locked_hits = ($samples | Measure-Object -Property locked_hits -Maximum).Maximum
    fenced_hits = ($samples | Measure-Object -Property fenced_hits -Maximum).Maximum
    data_bytes_first = $first.data_bytes
    data_bytes_last = $last.data_bytes
    data_bytes_per_hour = [math]::Round(($last.data_bytes - $first.data_bytes) / $hours, 1)
    working_set_mb_first = $first.working_set_mb
    working_set_mb_last = $last.working_set_mb
    working_set_mb_peak = ($samples | Measure-Object -Property working_set_mb -Maximum).Maximum
    evidence = $OutFile
}
$summaryPath = Join-Path (Split-Path -Parent $OutFile) 'soak-summary.json'
$report | ConvertTo-Json | Set-Content -LiteralPath $summaryPath -Encoding utf8
$report | ConvertTo-Json
