# Axon Gateway WebSocket 面调用脚本：握手、发送一帧、按窗口收帧并打印。
# 用于运行态排障与单个接口验证；不打印 token 原文。
#
# 用法：
#   pwsh -File gateway-ws.ps1 -Action ping
#   pwsh -File gateway-ws.ps1 -Action subscribe -SessionId <session-id> -SubscribeId "session:<session-id>"
#   pwsh -File gateway-ws.ps1 -Action subscribe -InteractionId <interaction-id> -SubscribeId "interaction:<interaction-id>" -ListenMs 5000
#   pwsh -File gateway-ws.ps1 -Action submit -SourceChannel jira -EventId AX-1024 `
#     -Content "事件正文" -ReplyChannel jira -ReplyDestination AX-1024
#
# 判定口径：输出以 OK:ws 或 ERR:ws 开头；退出码 0 表示握手成功且（订阅时）收到订阅确认，1 表示其他情况。
[CmdletBinding()]
param(
    [ValidateSet('ping', 'subscribe', 'submit')][string]$Action = 'ping',
    [string]$BaseUrl = '',
    [string]$Token = '',
    [string]$TokenFile = '',

    # 目标与订阅标识（-Action subscribe）
    [string]$SubscribeId = '',
    [string]$WorkspaceId = '',
    [string]$InteractionId = '',
    [string]$SessionId = '',
    [string]$ResumeCursor = '',

    # 事件字段（-Action submit）
    [string]$SourceChannel = '',
    [string]$EventId = '',
    [string]$Content = '',
    [string]$ReplyChannel = '',
    [string]$ReplyDestination = '',
    [string[]]$LocalFiles = @(),
    [long]$ObservedAtMs = 0,
    [int]$ExpiresInMinutes = 1440,

    [int]$ListenMs = 3000,
    [int]$ConnectTimeoutMs = 10000
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_axon-endpoint.ps1')

$WS_PATH = '/gateway/v1/ws'

function Write-WsResult([string]$Prefix, [string]$Text, [int]$ExitCode) {
    Write-Output ("{0} {1}" -f $Prefix, $Text)
    exit $ExitCode
}

if ($Action -eq 'subscribe' -and -not $SubscribeId) {
    Write-WsResult 'ERR:ws' 'subscribe 需要 -SubscribeId（连接内唯一，建议由目标推导）' 1
}
if ($Action -eq 'subscribe' -and -not $WorkspaceId) {
    Write-WsResult 'ERR:ws' 'subscribe 需要 -WorkspaceId（订阅帧的必填字段，缺省会被服务端拒为 invalid_json）' 1
}
if ($Action -eq 'subscribe' -and -not $InteractionId -and -not $SessionId) {
    Write-WsResult 'ERR:ws' 'subscribe 需要 -InteractionId 或 -SessionId' 1
}
if ($Action -eq 'submit' -and (-not $SourceChannel -or -not $EventId -or -not $Content -or -not $ReplyChannel -or -not $ReplyDestination)) {
    Write-WsResult 'ERR:ws' 'submit 需要 -SourceChannel、-EventId、-Content、-ReplyChannel、-ReplyDestination' 1
}

$base = Resolve-EndpointBaseUrl -Surface 'websocket' -BaseUrl $BaseUrl
if (-not $base) {
    Write-WsResult 'ERR:ws' '无法解析 WebSocket 基址：先确认 axon 在 PATH 中，或用 -BaseUrl 显式指定' 1
}
$target = $base + $WS_PATH
$resolved_token = Resolve-EndpointToken -Token $Token -TokenFile $TokenFile

$socket = [System.Net.WebSockets.ClientWebSocket]::new()
if ($resolved_token) {
    $socket.Options.SetRequestHeader('Authorization', "Bearer $resolved_token")
}
$connect_token = [System.Threading.CancellationTokenSource]::new($ConnectTimeoutMs).Token
try {
    $null = $socket.ConnectAsync([Uri]$target, $connect_token).GetAwaiter().GetResult()
} catch {
    Write-WsResult 'ERR:ws' ("握手失败 " + $target + "：" + $_.Exception.Message) 1
}

function Send-Frame([string]$Json) {
    $bytes = [Text.Encoding]::UTF8.GetBytes($Json)
    $segment = [ArraySegment[byte]]::new($bytes)
    $null = $socket.SendAsync($segment, [System.Net.WebSockets.WebSocketMessageType]::Text, $true,
        [System.Threading.CancellationToken]::None).GetAwaiter().GetResult()
}

if ($Action -eq 'ping') {
    Send-Frame '{"type":"ping"}'
} elseif ($Action -eq 'subscribe') {
    $frame = [ordered]@{ type = 'subscribe'; subscribeId = $SubscribeId }
    if ($WorkspaceId) { $frame['workspaceId'] = $WorkspaceId }
    if ($SessionId) { $frame['sessionId'] = $SessionId } elseif ($InteractionId) { $frame['interactionId'] = $InteractionId }
    if ($ResumeCursor) { $frame['resumeCursor'] = $ResumeCursor }
    Send-Frame ($frame | ConvertTo-Json -Depth 6 -Compress)
} else {
    $observed = if ($ObservedAtMs -gt 0) { $ObservedAtMs } else { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
    $frame = [ordered]@{
        type             = 'submit'
        sourceChannel    = $SourceChannel
        eventId          = $EventId
        observedAtMs     = [long]$observed
        expiresAtMs      = [long]($observed + ([long]$ExpiresInMinutes * 60000))
        content          = $Content
        replyChannel     = $ReplyChannel
        replyDestination = $ReplyDestination
    }
    if ($WorkspaceId) { $frame['workspaceId'] = $WorkspaceId }
    if ($LocalFiles.Count -gt 0) { $frame['localFiles'] = @($LocalFiles) }
    Send-Frame ($frame | ConvertTo-Json -Depth 6 -Compress)
}

$read_token = [System.Threading.CancellationTokenSource]::new($ListenMs).Token
$buffer = New-Object byte[] 262144
$frames = 0
$subscribed = $false
$failure = ''
while ($true) {
    try {
        $received = $socket.ReceiveAsync([ArraySegment[byte]]::new($buffer), $read_token).GetAwaiter().GetResult()
    } catch { break }
    if ($received.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) { break }
    $text = [Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count)
    $frames++
    Write-Output ("FRAME " + $text)
    if ($text -match '"type"\s*:\s*"subscribed"') { $subscribed = $true }
    if ($text -match '"type"\s*:\s*"error"') { $failure = $text }
    if (-not $received.EndOfMessage) { continue }
}

try {
    if ($Action -eq 'subscribe' -and $subscribed) {
        Send-Frame (([ordered]@{ type = 'unsubscribe'; subscribeId = $SubscribeId } | ConvertTo-Json -Compress))
    }
    $socket.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'done',
        [System.Threading.CancellationToken]::None).GetAwaiter().GetResult() | Out-Null
} catch { }
$socket.Dispose()

$summary = "target=$target frames=$frames"
if ($failure) { Write-WsResult 'ERR:ws' ($summary + ' ' + $failure) 1 }
if ($Action -eq 'subscribe' -and -not $subscribed) {
    Write-WsResult 'ERR:ws' ($summary + ' 未收到 subscribed 回执') 1
}
Write-WsResult 'OK:ws' $summary 0
