# CDP 文本输入辅助：按选择器点中文本框，插入文本，再按 Enter 提交。
#
# 用法：
#   pwsh -File tools/cdp-type.ps1 -Selector 'textarea' -Text '你好'
#   pwsh -File tools/cdp-type.ps1 -Selector 'textarea' -Text '你好' -Submit
param(
    [Parameter(Mandatory = $true)][string]$Selector,
    [Parameter(Mandatory = $true)][string]$Text,
    [switch]$Submit,
    [int]$Port = 9333,
    [int]$TimeoutMs = 20000
)

$ErrorActionPreference = 'Stop'

function Write-Result([string]$Message) {
    Write-Output $Message
    exit 0
}

try {
    $targets = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/json" -TimeoutSec 5
}
catch {
    Write-Result "ERR:CDP 不可达（$Port）：$($_.Exception.Message)"
}

$page = $targets | Where-Object { $_.type -eq 'page' } | Select-Object -First 1
if ($null -eq $page) { Write-Result 'ERR:没有可用的 page target' }

$ws = New-Object System.Net.WebSockets.ClientWebSocket
$ct = [System.Threading.CancellationToken]::None
try {
    $ws.ConnectAsync([Uri]$page.webSocketDebuggerUrl, $ct).GetAwaiter().GetResult() | Out-Null
}
catch {
    Write-Result "ERR:WebSocket 连接失败：$($_.Exception.Message)"
}

$script:nextId = 0
$script:buffer = New-Object byte[] 1048576

function Invoke-Cdp([string]$Method, $Params) {
    $script:nextId++
    $id = $script:nextId
    $payload = @{ id = $id; method = $Method; params = $Params } | ConvertTo-Json -Depth 12 -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
    $ws.SendAsync([ArraySegment[byte]]::new($bytes), [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).
        GetAwaiter().GetResult() | Out-Null

    $deadline = (Get-Date).AddMilliseconds($TimeoutMs)
    while ((Get-Date) -lt $deadline) {
        $segment = [ArraySegment[byte]]::new($script:buffer)
        $received = $ws.ReceiveAsync($segment, $ct).GetAwaiter().GetResult()
        if ($received.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) {
            throw '连接被对端关闭'
        }
        $text = [System.Text.Encoding]::UTF8.GetString($script:buffer, 0, $received.Count)
        $obj = $text | ConvertFrom-Json
        if ($null -ne $obj.id -and [int]$obj.id -eq $id) { return $obj }
    }
    throw "等待 $Method 回执超时"
}

# 1) 定位并点中目标输入框。
$locateScript = @'
(() => {
  const node = document.querySelector(__SELECTOR__);
  if (!node) return JSON.stringify({ ok: false, reason: 'no-match' });
  node.scrollIntoView({ block: 'center' });
  node.focus();
  const rect = node.getBoundingClientRect();
  return JSON.stringify({
    ok: true,
    x: Math.round(rect.left + rect.width / 2),
    y: Math.round(rect.top + rect.height / 2),
  });
})()
'@
$locateScript = $locateScript.Replace('__SELECTOR__', ($Selector | ConvertTo-Json -Compress))
$located = Invoke-Cdp 'Runtime.evaluate' @{ expression = $locateScript; returnByValue = $true; awaitPromise = $true }
if ($located.result.exceptionDetails) { Write-Result "ERR:定位失败：$($located.result.exceptionDetails.text)" }
$info = $located.result.result.value | ConvertFrom-Json
if (-not $info.ok) { Write-Result "ERR:未命中输入框（$Selector reason=$($info.reason)）" }

foreach ($type in @('mouseMoved', 'mousePressed', 'mouseReleased')) {
    $buttons = if ($type -eq 'mousePressed') { 1 } else { 0 }
    Invoke-Cdp 'Input.dispatchMouseEvent' @{
        type = $type; x = $info.x; y = $info.y; button = 'left'; clickCount = 1; buttons = $buttons
    } | Out-Null
}

# 2) 插入文本：input 事件由 insertText 合成为真实输入，React 受控组件能收到。
Invoke-Cdp 'Input.insertText' @{ text = $Text } | Out-Null

if ($Submit) {
    foreach ($type in @('keyDown', 'keyUp')) {
        Invoke-Cdp 'Input.dispatchKeyEvent' @{
            type                  = $type
            key                   = 'Enter'
            code                  = 'Enter'
            windowsVirtualKeyCode = 13
            nativeVirtualKeyCode  = 13
            text                  = if ($type -eq 'keyDown') { "`r" } else { '' }
        } | Out-Null
    }
}

# 3) 回读输入框内容，确认文本真的进去了。
$readBack = Invoke-Cdp 'Runtime.evaluate' @{
    expression    = "(() => { const n = document.querySelector($($Selector | ConvertTo-Json -Compress)); return n ? (n.value || '') : '__missing__'; })()"
    returnByValue = $true
}

try { $ws.Dispose() } catch { }

Write-Output "OK:typed=$($readBack.result.result.value)"
