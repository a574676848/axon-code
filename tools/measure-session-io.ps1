<#
.SYNOPSIS
  会话首屏在并发执行下的延迟复现脚本。

.DESCRIPTION
  同时拉起若干条 axon run 制造写入压力，期间反复并发请求桌面端加载会话所需的
  四个只读端点（会话清单、历史首页、后台作业、运行态证据），按端点分桶输出单请求
  p50 / p95 / max，另给出一次并发四请求的整体墙钟。

  只统计「至少有一条会话在跑」的采样窗口。历史首页在会话运行中必然走冷路径
  （缓存对运行中会话旁路），因此这里的数字是首屏的冷路径上界；空闲会话的命中路径
  不在本脚本口径内。

.PARAMETER Runs
  并发 run 的条数，默认 4。

.PARAMETER Seconds
  采样时长上限，默认 30 秒。

.PARAMETER Gateway
  网关地址，默认取本机 127.0.0.1:3143，可用参数覆盖。

.PARAMETER Session
  观察窗口用的会话标识，默认取最近一条有历史的会话。

.NOTES
  token 从 AXON_GATEWAY_API_TOKEN 或 $HOME/.axon/.env 读取，不接受命令行明文。
#>
param(
  [int]$Runs = 4,
  [int]$Seconds = 30,
  [string]$Gateway = 'http://127.0.0.1:3143',
  [string]$Prompt = 'reply with the single word OK',
  [string]$Session = ''
)

$ErrorActionPreference = 'Stop'

function Resolve-Token {
  if ($env:AXON_GATEWAY_API_TOKEN) { return $env:AXON_GATEWAY_API_TOKEN }
  $envFile = Join-Path $HOME '.axon/.env'
  if (-not (Test-Path $envFile)) { throw "找不到网关 token：请设置 AXON_GATEWAY_API_TOKEN" }
  $line = Get-Content $envFile | Where-Object { $_ -match '^AXON_GATEWAY_API_TOKEN=' } | Select-Object -First 1
  if (-not $line) { throw "找不到网关 token：$envFile 中没有 AXON_GATEWAY_API_TOKEN" }
  return ($line -split '=', 2)[1].Trim()
}

function Resolve-AxonCommand {
  $installRoot = if ($env:AXON_INSTALL_ROOT) { $env:AXON_INSTALL_ROOT } else { Join-Path $HOME '.axon/bin' }
  $command = Join-Path $installRoot 'axon.cmd'
  if (-not (Test-Path $command)) { throw "找不到 axon 入口：$command" }
  return $command
}

function Measure-Percentile {
  param([double[]]$Values, [double]$Percentile)
  if ($Values.Count -eq 0) { return [double]::NaN }
  $sorted = $Values | Sort-Object
  $index = [int][math]::Floor(($sorted.Count - 1) * $Percentile)
  return $sorted[$index]
}

function Format-Bucket {
  param([string]$Name, [System.Collections.Generic.List[double]]$Values)
  if ($Values.Count -eq 0) { return "{0}: 无样本" -f $Name }
  $array = $Values.ToArray()
  return ("{0}: n={1} p50={2:N1}ms p95={3:N1}ms max={4:N1}ms" -f `
    $Name,
    $array.Count,
    (Measure-Percentile $array 0.5),
    (Measure-Percentile $array 0.95),
    ($array | Measure-Object -Maximum).Maximum)
}

$token = Resolve-Token
$axon = Resolve-AxonCommand
$api = "$($Gateway.TrimEnd('/'))/api/v1"
$version = (& $axon --version 2>&1 | Select-Object -First 1)

if (-not $Session) {
  $state = (& curl.exe -s -H "Authorization: Bearer $token" "$api/sessions") | ConvertFrom-Json
  $Session = ($state.sessions | Select-Object -First 1).sessionId
  if (-not $Session) { throw '没有可观察的会话：请先用 axon run 建一条会话' }
}

Write-Output "version=$version gateway=$Gateway runs=$Runs seconds=$Seconds session=$Session"

$endpoints = @('sessions', 'interactions', 'jobs', 'live')
$buckets = @{}
foreach ($name in $endpoints) {
  $buckets[$name] = New-Object System.Collections.Generic.List[double]
}

$jobs = 1..$Runs | ForEach-Object {
  Start-Job -ScriptBlock { param($command, $text) & $command run $text } -ArgumentList $axon, $Prompt
}

$walls = New-Object System.Collections.Generic.List[double]
$activeWindows = 0
$droppedWindows = 0
$atLeastRunsWindows = 0
$elapsed = [System.Diagnostics.Stopwatch]::StartNew()
while ($elapsed.Elapsed.TotalSeconds -lt $Seconds) {
  $state = (& curl.exe -s -H "Authorization: Bearer $token" "$api/sessions") | ConvertFrom-Json
  $running = ($state.sessions | Where-Object { $_.running }).Count
  if ($running -ge 1) {
    $wall = [System.Diagnostics.Stopwatch]::StartNew()
    # curl 的 -w 按传输完成顺序输出，因此每行带 url_effective，按 URL 归桶而不是按下标。
    $perCall = & curl.exe -s -o NUL -o NUL -o NUL -o NUL -w "%{url_effective} %{time_total}`n" --parallel --parallel-max 4 `
      -H "Authorization: Bearer $token" `
      "$api/sessions" `
      "$api/sessions/$Session/interactions" `
      "$api/sessions/$Session/jobs" `
      "$api/sessions/$Session/live"
    $wall.Stop()
    $walls.Add($wall.Elapsed.TotalMilliseconds)
    $sample = @{}
    foreach ($line in $perCall) {
      if ($line -match '^(\S+)\s+([\d.]+)$') {
        $sample[$Matches[1]] = [double]$Matches[2] * 1000
      }
    }
    $resolved = 0
    foreach ($name in $endpoints) {
      $key = if ($name -eq 'sessions') { "$api/sessions" } else { "$api/sessions/$Session/$name" }
      if ($sample.ContainsKey($key)) {
        $buckets[$name].Add($sample[$key])
        $resolved++
      }
    }
    if ($resolved -ne $endpoints.Count) { $droppedWindows++ }
    if ($running -ge $Runs) { $atLeastRunsWindows++ }
    $activeWindows++
  }
  if ((Get-Job -State Running).Count -eq 0 -and $activeWindows -gt 0) { break }
  Start-Sleep -Milliseconds 350
}

Get-Job | Stop-Job -PassThru | Remove-Job -Force

$wallArray = $walls.ToArray()
if ($wallArray.Count -eq 0) {
  Write-Output '没有抓到活跃窗口：run 可能启动失败或很快就结束了'
  exit 1
}

Write-Output ("active_windows={0} dropped_windows={1} at_least_runs_running={2} wall_p50={3:N1}ms wall_p95={4:N1}ms wall_max={5:N1}ms" -f `
  $activeWindows,
  $droppedWindows,
  $atLeastRunsWindows,
  (Measure-Percentile $wallArray 0.5),
  (Measure-Percentile $wallArray 0.95),
  ($wallArray | Measure-Object -Maximum).Maximum)
foreach ($name in $endpoints) {
  Write-Output (Format-Bucket $name $buckets[$name])
}
