[CmdletBinding()]
param(
    [string]$AxonExecutable = "axon",
    [Parameter(Mandatory = $true)]
    [string]$ConfigPath,
    [Parameter(Mandatory = $true)]
    [string]$WorkingDirectory,
    [string]$Model,
    [string]$SessionId = "prompt-cache-benchmark",
    [ValidateRange(2, 100)]
    [int]$Turns = 5,
    [string]$Prompt = "继续检查当前工作区，并用一句话报告进展。"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$BenchmarkGateValue = "1"
$UsageEventType = "usageUpdate"
$AssistantEventType = "assistantMessage"
$TailLineCount = 20

if ($env:AXON_PROMPT_CACHE_BENCHMARK -ne $BenchmarkGateValue) {
    throw "live benchmark 默认关闭；请先设置 AXON_PROMPT_CACHE_BENCHMARK=1。"
}

$ResolvedConfigPath = (Resolve-Path -LiteralPath $ConfigPath).Path
$ResolvedWorkingDirectory = (Resolve-Path -LiteralPath $WorkingDirectory).Path
$ResolvedAxonExecutable = if (Test-Path -LiteralPath $AxonExecutable) {
    (Resolve-Path -LiteralPath $AxonExecutable).Path
}
else {
    (Get-Command -Name $AxonExecutable -ErrorAction Stop).Source
}

$Results = [System.Collections.Generic.List[object]]::new()
for ($Turn = 1; $Turn -le $Turns; $Turn += 1) {
    $Arguments = @(
        "--config", $ResolvedConfigPath,
        "--cwd", $ResolvedWorkingDirectory,
        "run",
        "--format", "json",
        "--session", $SessionId
    )
    if (-not [string]::IsNullOrWhiteSpace($Model)) {
        $Arguments += @("--model", $Model)
    }
    $Arguments += $Prompt

    $Stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    $Usage = $null
    $FirstTokenMs = $null
    $ObservedSessionId = $null
    $RawOutput = [System.Collections.Generic.List[string]]::new()

    & $ResolvedAxonExecutable @Arguments 2>&1 | ForEach-Object {
        $Line = $_.ToString()
        $RawOutput.Add($Line)
        try {
            $Event = $Line | ConvertFrom-Json -ErrorAction Stop
        }
        catch {
            return
        }
        $WireEventProperty = $Event.PSObject.Properties['wireEvent']
        $WireEvent = if ($null -ne $WireEventProperty -and $null -ne $WireEventProperty.Value) {
            $WireEventProperty.Value
        }
        else {
            $Event
        }
        $SessionProperty = $WireEvent.PSObject.Properties['session']
        if ($null -ne $SessionProperty -and $null -ne $SessionProperty.Value) {
            $SessionIdProperty = $SessionProperty.Value.PSObject.Properties['sessionId']
            if ($null -ne $SessionIdProperty -and $null -ne $SessionIdProperty.Value) {
                $ObservedSessionId = [string]$SessionIdProperty.Value
            }
        }
        $KindProperty = $WireEvent.PSObject.Properties['kind']
        if ($null -eq $KindProperty -or $null -eq $KindProperty.Value) {
            return
        }
        $Kind = $KindProperty.Value
        $TypeProperty = $Kind.PSObject.Properties['type']
        $PayloadProperty = $Kind.PSObject.Properties['payload']
        if ($null -eq $TypeProperty -or $null -eq $PayloadProperty) {
            return
        }
        $EventType = [string]$TypeProperty.Value
        $Payload = $PayloadProperty.Value
        if ($EventType -eq $UsageEventType) {
            $Usage = $Payload
        }
        $DeltaProperty = $Payload.PSObject.Properties['delta']
        $ContentProperty = $Payload.PSObject.Properties['content']
        if ($null -eq $FirstTokenMs -and
            $EventType -eq $AssistantEventType -and
            $null -ne $DeltaProperty -and $DeltaProperty.Value -eq $true -and
            $null -ne $ContentProperty -and
            -not [string]::IsNullOrEmpty([string]$ContentProperty.Value)) {
            $FirstTokenMs = $Stopwatch.ElapsedMilliseconds
        }
    }
    $ExitCode = $LASTEXITCODE
    $Stopwatch.Stop()

    if ($ExitCode -ne 0) {
        $Tail = ($RawOutput | Select-Object -Last $TailLineCount) -join [Environment]::NewLine
        throw "第 $Turn 轮 benchmark 失败，exitCode=$ExitCode。`n$Tail"
    }

    $Anomalies = [System.Collections.Generic.List[string]]::new()
    if ($null -eq $Usage) {
        $Anomalies.Add("missing_usage")
        $InputTokens = 0L
        $CacheReadTokens = 0L
        $CacheWriteTokens = 0L
    }
    else {
        $InputTokens = [long]$Usage.inputTokens
        $CacheReadTokens = [long]$Usage.cachedInputTokens
        $CacheWriteTokens = [long]$Usage.cacheWriteInputTokens
    }
    $TotalInputTokens = $InputTokens + $CacheReadTokens + $CacheWriteTokens
    if ($CacheReadTokens -gt $TotalInputTokens) {
        $Anomalies.Add("cache_read_exceeds_total_input")
    }
    if ($null -ne $ObservedSessionId -and $ObservedSessionId -ne $SessionId) {
        $Anomalies.Add("session_id_mismatch")
    }
    $HitRate = if ($TotalInputTokens -eq 0) {
        0.0
    }
    else {
        [Math]::Round(($CacheReadTokens * 100.0) / $TotalInputTokens, 2)
    }

    $Result = [ordered]@{
        type = "prompt_cache_benchmark_turn"
        turn = $Turn
        sessionId = $SessionId
        model = $Model
        totalInputTokens = $TotalInputTokens
        uncachedInputTokens = $InputTokens
        cacheReadInputTokens = $CacheReadTokens
        cacheWriteInputTokens = $CacheWriteTokens
        cacheHitRatePercent = $HitRate
        durationMs = $Stopwatch.ElapsedMilliseconds
        ttftMs = $FirstTokenMs
        anomalies = $Anomalies
    }
    $Results.Add([pscustomobject]$Result)
    $Result | ConvertTo-Json -Compress -Depth 6
}

[ordered]@{
    type = "prompt_cache_benchmark_summary"
    sessionId = $SessionId
    turns = $Turns
    hitRateCurvePercent = @($Results | ForEach-Object { $_.cacheHitRatePercent })
    anomalyCount = @($Results | ForEach-Object { $_.anomalies } | Where-Object { $_ }).Count
} | ConvertTo-Json -Compress -Depth 6
