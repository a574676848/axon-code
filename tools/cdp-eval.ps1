# CDP 一次性求值辅助：连接 WebView2 调试端口，执行一段表达式并返回结果。
# 每次调用独立建连、带超时；连接失效时立刻返回错误而不是空转。
param(
  [Parameter(Mandatory = $true)][string]$Expression,
  [int]$Port = 9333,
  [int]$TimeoutMs = 15000
)

$ErrorActionPreference = 'Stop'

try {
  $targets = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/json" -TimeoutSec 5
} catch {
  Write-Output "ERR:CDP 不可达（$Port）：$($_.Exception.Message)"
  exit 0
}

$page = $targets | Where-Object { $_.type -eq 'page' } | Select-Object -First 1
if ($null -eq $page) {
  Write-Output 'ERR:没有可用的 page target'
  exit 0
}

$ws = New-Object System.Net.WebSockets.ClientWebSocket
$ct = [System.Threading.CancellationToken]::None
try {
  $ws.ConnectAsync([Uri]$page.webSocketDebuggerUrl, $ct).GetAwaiter().GetResult() | Out-Null
} catch {
  Write-Output "ERR:WebSocket 连接失败：$($_.Exception.Message)"
  exit 0
}

$payload = @{
  id     = 1
  method = 'Runtime.evaluate'
  params = @{ expression = $Expression; returnByValue = $true; awaitPromise = $true }
} | ConvertTo-Json -Depth 12 -Compress

try {
  $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
  $ws.SendAsync([ArraySegment[byte]]::new($bytes), [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).GetAwaiter().GetResult() | Out-Null

  $buffer = New-Object byte[] 262144
  $deadline = (Get-Date).AddMilliseconds($TimeoutMs)
  while ((Get-Date) -lt $deadline) {
    $segment = [ArraySegment[byte]]::new($buffer)
    $received = $ws.ReceiveAsync($segment, $ct).GetAwaiter().GetResult()
    if ($received.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) {
      Write-Output 'ERR:连接被对端关闭'
      break
    }
    $text = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count)
    if ($text -notmatch '"id"\s*:\s*1') { continue }
    $obj = $text | ConvertFrom-Json
    if ($obj.result.exceptionDetails) {
      Write-Output "ERR:$($obj.result.exceptionDetails.text)"
    } else {
      Write-Output ([string]$obj.result.result.value)
    }
    break
  }
} catch {
  Write-Output "ERR:求值失败：$($_.Exception.Message)"
} finally {
  try { $ws.Dispose() } catch { }
}
