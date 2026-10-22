# Axon HTTP 接口调用脚本：解析监听地址与凭据，调用 Gateway 面或 headless server 面的路由，
# 输出状态码与响应体。不打印 token 原文，不在本机留下调用记录。
#
# 用法：
#   pwsh -File gateway-http.ps1 -Action overview -Query "period=7d"
#   pwsh -File gateway-http.ps1 -Action lanes -Query "workspace=<id>"
#   pwsh -File gateway-http.ps1 -Action event -SourceChannel jira -EventId AX-1024 `
#     -Content "事件正文" -ReplyChannel jira -ReplyDestination AX-1024
#   pwsh -File gateway-http.ps1 -Action interaction -InteractionId <id>
#   pwsh -File gateway-http.ps1 -Action raw -Method GET -Path "/api/v1/crons"
#   pwsh -File gateway-http.ps1 -Surface serve -BaseUrl "http://127.0.0.1:3000" -Action health
#
# 判定口径：输出以 OK:<状态码> 或 ERR:<状态码> 开头；退出码 0 表示 2xx，1 表示其他情况。
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet(
        'health', 'ready', 'models', 'sessions',
        'workspaces', 'lanes',
        'overview', 'realtime', 'distribution', 'diagnostics',
        'audit-logs', 'audit-export',
        'pricing-rules', 'pricing-auto-sync', 'pricing-sync', 'pricing-recalculate',
        'crons',
        'interaction', 'event', 'raw')]
    [string]$Action,

    [ValidateSet('gateway', 'serve')][string]$Surface = 'gateway',
    [string]$BaseUrl = '',
    [string]$Token = '',
    [string]$TokenFile = '',
    [string]$Method = 'GET',
    [string]$Path = '',
    [string]$Query = '',
    [string]$Body = '',
    [string]$BodyFile = '',

    # 通用事件字段（-Action event）
    [string]$SourceChannel = '',
    [string]$EventId = '',
    [string]$WorkspaceId = '',
    [string]$Content = '',
    [string]$ReplyChannel = '',
    [string]$ReplyDestination = '',
    [string[]]$LocalFiles = @(),
    [long]$ObservedAtMs = 0,
    [int]$ExpiresInMinutes = 1440,

    [string]$InteractionId = '',
    [int]$TimeoutSec = 15
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_axon-endpoint.ps1')

function Write-Result([string]$Prefix, [string]$Url, [string]$Text, [int]$ExitCode) {
    Write-Output ("{0} {1} {2}" -f $Prefix, $Url, $Text)
    exit $ExitCode
}

function New-EventBody {
    if (-not $SourceChannel -or -not $EventId -or -not $Content -or -not $ReplyChannel -or -not $ReplyDestination) {
        Write-Result 'ERR:0' '(未发送)' 'event 需要 -SourceChannel、-EventId、-Content、-ReplyChannel、-ReplyDestination' 1
    }
    $observed = if ($ObservedAtMs -gt 0) { $ObservedAtMs } else { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
    $envelope = [ordered]@{
        sourceChannel    = $SourceChannel
        eventId          = $EventId
        observedAtMs     = [long]$observed
        expiresAtMs      = [long]($observed + ([long]$ExpiresInMinutes * 60000))
        content          = $Content
        replyChannel     = $ReplyChannel
        replyDestination = $ReplyDestination
    }
    if ($WorkspaceId) { $envelope['workspaceId'] = $WorkspaceId }
    if ($LocalFiles.Count -gt 0) { $envelope['localFiles'] = @($LocalFiles) }
    return ($envelope | ConvertTo-Json -Depth 6 -Compress)
}

function Resolve-Route {
    $prefix = if ($Surface -eq 'gateway') { '/api/v1' } else { '/v1' }
    switch ($Action) {
        'health' { return @('GET', '/v1/health') }
        'ready' { return @('GET', '/v1/ready') }
        'models' { return @('GET', '/v1/models') }
        'sessions' { return @('GET', '/v1/sessions') }
        'workspaces' { return @('GET', "$prefix/workspaces") }
        'lanes' { return @('GET', "$prefix/lanes") }
        'overview' { return @('GET', '/api/v1/observability/overview') }
        'realtime' { return @('GET', '/api/v1/observability/realtime') }
        'distribution' { return @('GET', '/api/v1/observability/distribution') }
        'diagnostics' { return @('GET', '/api/v1/observability/diagnostics') }
        'audit-logs' { return @('GET', '/api/v1/observability/audit/logs') }
        'audit-export' { return @('GET', '/api/v1/observability/audit/export') }
        'pricing-rules' { return @('GET', '/api/v1/observability/pricing/rules') }
        'pricing-auto-sync' { return @('GET', '/api/v1/observability/pricing/auto-sync') }
        'pricing-sync' { return @('POST', '/api/v1/observability/pricing/sync') }
        'pricing-recalculate' { return @('POST', '/api/v1/observability/pricing/recalculate') }
        'crons' { return @('GET', '/api/v1/crons') }
        'interaction' {
            if (-not $InteractionId) { Write-Result 'ERR:0' '(未发送)' 'interaction 需要 -InteractionId' 1 }
            return @('GET', "/gateway/v1/interactions/$InteractionId")
        }
        'event' { return @('POST', '/gateway/v1/events') }
        'raw' {
            if (-not $Path) { Write-Result 'ERR:0' '(未发送)' 'raw 需要 -Path' 1 }
            return @($Method.ToUpperInvariant(), $Path)
        }
    }
}

$resolved_base = Resolve-EndpointBaseUrl -Surface $Surface -BaseUrl $BaseUrl
if (-not $resolved_base) {
    Write-Result 'ERR:0' '(未发送)' '无法解析基址：先确认 axon 在 PATH 中，或用 -BaseUrl 显式指定' 1
}

$route = Resolve-Route
$resolved_method = $route[0]
$resolved_path = $route[1]

if ($Action -eq 'event') {
    $Body = New-EventBody
} elseif (-not $Body -and $BodyFile -and (Test-Path -LiteralPath $BodyFile -PathType Leaf)) {
    $Body = [IO.File]::ReadAllText($BodyFile)
}

$target = $resolved_base + $resolved_path
if ($Query) { $target = $target + '?' + $Query.TrimStart('?') }

$resolved_token = Resolve-EndpointToken -Token $Token -TokenFile $TokenFile
if (-not $resolved_token) {
    Write-Output 'WARN 未解析到 API token：实例未配置 token 时，Gateway 的全部路由会返回 401。'
}

$client = [System.Net.Http.HttpClient]::new()
$client.Timeout = [TimeSpan]::FromSeconds($TimeoutSec)
try {
    $request = [System.Net.Http.HttpRequestMessage]::new(
        [System.Net.Http.HttpMethod]::new($resolved_method), $target)
    if ($resolved_token) {
        $request.Headers.TryAddWithoutValidation('Authorization', "Bearer $resolved_token") | Out-Null
    }
    if ($Body) {
        $request.Content = [System.Net.Http.StringContent]::new(
            $Body, [Text.Encoding]::UTF8, 'application/json')
    }
    try {
        $response = $client.Send($request)
    } catch {
        Write-Result 'ERR:0' $target ("请求失败：" + $_.Exception.Message) 1
    }
    $status = [int]$response.StatusCode
    $text = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    if ($text.Length -gt 20000) { $text = $text.Substring(0, 20000) + '...<已截断>' }
    $ok = $status -ge 200 -and $status -lt 300
    $prefix = if ($ok) { 'OK:' + $status } else { 'ERR:' + $status }
    Write-Result $prefix $target $text $(if ($ok) { 0 } else { 1 })
} finally {
    $client.Dispose()
}
