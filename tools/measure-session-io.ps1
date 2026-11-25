<#
.SYNOPSIS
  会话首屏在并发执行下的延迟复现脚本。

.DESCRIPTION
  同时拉起若干条 axon run 制造写入压力，期间反复并发请求桌面端加载会话所需的
  四个只读端点（会话清单、历史首页、后台作业、运行态证据），输出 wall 与单请求
  的 p50 / p95 / max。只统计「至少有一条会话在跑」的采样窗口，避免把空闲态混进
  结果。

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

$token = Resolve-Token
$axon = Resolve-AxonCommand
$api = "$($Gateway.TrimEnd('/'))/api/v1"

if (-not $Session) {
  $state = (& curl.exe -s -H "Authorization: Bearer $token" "$api/sessions") | ConvertFrom-Json
  $Session = ($state.sessions | Select-Object -First 1).sessionId
  if (-not $Session) { throw '没有可观察的会话：请先用 axon run 建一条会话' }
}

Write-Output "gateway=$Gateway runs=$Runs seconds=$Seconds session=$Session"

$jobs = 1..$Runs | ForEach-Object {
  Start-Job -ScriptBlock { param($command, $text) & $command run $text } -ArgumentList $axon, $Prompt
}

$walls = New-Object System.Collections.Generic.List[double]
$perRequest = New-Object System.Collections.Generic.List[double]
$activeWindows = 0
$elapsed = [System.Diagnostics.Stopwatch]::StartNew()
while ($elapsed.Elapsed.TotalSeconds -lt $Seconds) {
  $state = (& curl.exe -s -H "Authorization: Bearer $token" "$api/sessions") | ConvertFrom-Json
  $running = ($state.sessions | Where-Object { $_.running }).Count
  if ($running -ge 1) {
    $wall = [System.Diagnostics.Stopwatch]::StartNew()
    $perCall = & curl.exe -s -o NUL -w "%{time_total}`n" --parallel --parallel-max 4 `
      -H "Authorization: Bearer $token" `
      "$api/sessions" `
      "$api/sessions/$Session/interactions" `
      "$api/sessions/$Session/jobs" `
      "$api/sessions/$Session/live"
    $wall.Stop()
    $walls.Add($wall.Elapsed.TotalMilliseconds)
    foreach ($line in $perCall) {
      if ($line -match '^\d') { $perRequest.Add([double]$line * 1000) }
    }
    $activeWindows++
  }
  if ((Get-Job -State Running).Count -eq 0 -and $activeWindows -gt 0) { break }
  Start-Sleep -Milliseconds 350
}

Get-Job | Stop-Job -PassThru | Remove-Job -Force

$wallArray = $walls.ToArray()
$requestArray = $perRequest.ToArray()
if ($wallArray.Count -eq 0) {
  Write-Output '没有抓到活跃窗口：run 可能启动失败或很快就结束了'
  exit 1
}

Write-Output ("active_windows={0} wall_p50={1:N1}ms wall_p95={2:N1}ms wall_max={3:N1}ms" -f `
  $activeWindows,
  (Measure-Percentile $wallArray 0.5),
  (Measure-Percentile $wallArray 0.95),
  ($wallArray | Measure-Object -Maximum).Maximum)
Write-Output ("requests={0} request_p50={1:N1}ms request_p95={2:N1}ms request_max={3:N1}ms" -f `
  $requestArray.Count,
  (Measure-Percentile $requestArray 0.5),
  (Measure-Percentile $requestArray 0.95),
  ($requestArray | Measure-Object -Maximum).Maximum)
