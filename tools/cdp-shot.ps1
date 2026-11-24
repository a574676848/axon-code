# CDP 截图辅助：连接 WebView2 调试端口，截取当前可见视口并存成 PNG。
#
# 与 cdp-eval 同一套建连方式：每次调用独立建连、带超时，失败时输出 ERR: 行而不是抛异常。
# 需要整页时传 -FullPage，脚本会先量 documentElement 的尺寸再截。
param(
  [Parameter(Mandatory = $true)][string]$OutPath,
  [int]$Port = 9333,
  [int]$TimeoutMs = 20000,
  [switch]$FullPage
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

# 发一条请求并等到同 id 的响应；返回解析后的对象，超时或异常返回 $null。
function Send-Cdp {
  param([hashtable]$Message, [int]$ExpectedId, [int]$WaitMs)
  $payload = $Message | ConvertTo-Json -Depth 12 -Compress
  $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
  $ws.SendAsync([ArraySegment[byte]]::new($bytes), [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).GetAwaiter().GetResult() | Out-Null

  $buffer = New-Object byte[] 8388608
  $deadline = (Get-Date).AddMilliseconds($WaitMs)
  while ((Get-Date) -lt $deadline) {
    $segment = [ArraySegment[byte]]::new($buffer)
    $received = $ws.ReceiveAsync($segment, $ct).GetAwaiter().GetResult()
    if ($received.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) { return $null }
    $text = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count)
    if ($text -notmatch ('"id"\s*:\s*' + $ExpectedId + '\b')) { continue }
    return ($text | ConvertFrom-Json)
  }
  return $null
}

try {
  $sizeCommand = @{
    id     = 1
    method = 'Runtime.evaluate'
    params = @{
      expression    = 'JSON.stringify({ w: document.documentElement.clientWidth, h: document.documentElement.scrollHeight, dpr: window.devicePixelRatio })'
      returnByValue = $true
    }
  }
  $sizeReply = Send-Cdp -Message $sizeCommand -ExpectedId 1 -WaitMs $TimeoutMs
  if ($null -eq $sizeReply) {
    Write-Output 'ERR:量取页面尺寸超时'
    exit 0
  }
  $size = $sizeReply.result.result.value | ConvertFrom-Json

  $shotParams = @{ format = 'png'; captureBeyondViewport = [bool]$FullPage }
  if ($FullPage) {
    $shotParams['clip'] = @{
      x      = 0
      y      = 0
      width  = [double]$size.w
      height = [double]$size.h
      scale  = 1
    }
  }

  $shotReply = Send-Cdp -Message @{ id = 2; method = 'Page.captureScreenshot'; params = $shotParams } -ExpectedId 2 -WaitMs $TimeoutMs
  if ($null -eq $shotReply -or $null -eq $shotReply.result.data) {
    Write-Output 'ERR:截图失败或超时'
    exit 0
  }

  $dir = Split-Path -Parent $OutPath
  if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
  [System.IO.File]::WriteAllBytes($OutPath, [Convert]::FromBase64String($shotReply.result.data))
  Write-Output ("OK:{0} ({1}x{2})" -f $OutPath, $size.w, $size.h)
} catch {
  Write-Output "ERR:截图失败：$($_.Exception.Message)"
} finally {
  try { $ws.Dispose() } catch { }
}
