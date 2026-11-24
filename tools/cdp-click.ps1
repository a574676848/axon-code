# CDP 真实指针序列辅助：按选择器定位元素，先滚动到可见，再派发
# mousedown → mouseup → click 三连（dispatched 真实指针事件，不是 Node.click()）。
#
# 用法：
#   pwsh -File tools/cdp-click.ps1 -Selector 'button[title="子代理"]'
#   pwsh -File tools/cdp-click.ps1 -Selector 'button' -TextMatch '返回会话'
#   pwsh -File tools/cdp-click.ps1 -Selector '.message-body' -Button right
#   pwsh -File tools/cdp-click.ps1 -Selector 'span' -TextMatch '应用版本' -ClickCount 2
param(
    [Parameter(Mandatory = $true)][string]$Selector,
    [string]$TextMatch = "",
    # 指针按键：left 是常规点击，right 用来验证右键菜单。
    [ValidateSet('left', 'right')][string]$Button = 'left',
    # 连击次数：2 会在一次单击之后补一次 clickCount=2 的按下抬起（真实双击）。
    [ValidateRange(1, 2)][int]$ClickCount = 1,
    [int]$Port = 9333,
    [int]$TimeoutMs = 15000
)

$ErrorActionPreference = 'Stop'

function Write-Result([string]$Text) {
    Write-Output $Text
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
$buffer = New-Object byte[] 1048576

function Invoke-Cdp([string]$Method, $Params) {
    $script:nextId++
    $id = $script:nextId
    $payload = @{ id = $id; method = $Method; params = $Params } | ConvertTo-Json -Depth 12 -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
    $ws.SendAsync([ArraySegment[byte]]::new($bytes), [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).
        GetAwaiter().GetResult() | Out-Null

    $deadline = (Get-Date).AddMilliseconds($TimeoutMs)
    while ((Get-Date) -lt $deadline) {
        $segment = [ArraySegment[byte]]::new($buffer)
        $received = $ws.ReceiveAsync($segment, $ct).GetAwaiter().GetResult()
        if ($received.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) {
            throw '连接被对端关闭'
        }
        $text = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count)
        $obj = $text | ConvertFrom-Json
        if ($null -ne $obj.id -and [int]$obj.id -eq $id) { return $obj }
    }
    throw "等待 $Method 回执超时"
}

# 1) 定位：滚动到可见并取中心点与命中信息。
$locateScript = @'
(() => {
  const selector = __SELECTOR__;
  const textMatch = __TEXT__;
  let nodes = Array.from(document.querySelectorAll(selector));
  if (textMatch) {
    nodes = nodes.filter((node) => (node.innerText || '').includes(textMatch));
  }
  if (nodes.length === 0) {
    return JSON.stringify({ ok: false, reason: 'no-match', count: 0 });
  }
  const node = nodes[0];
  node.scrollIntoView({ block: 'center', inline: 'center' });
  const rect = node.getBoundingClientRect();
  const x = Math.round(rect.left + rect.width / 2);
  const y = Math.round(rect.top + rect.height / 2);
  const hit = document.elementFromPoint(x, y);
  return JSON.stringify({
    ok: true,
    count: nodes.length,
    x, y,
    w: Math.round(rect.width), h: Math.round(rect.height),
    text: (node.innerText || '').slice(0, 60),
    hitInside: !!(hit && (node === hit || node.contains(hit))),
  });
})()
'@
$escapedSelector = $Selector | ConvertTo-Json -Compress
$escapedText = $TextMatch | ConvertTo-Json -Compress
$locateScript = $locateScript.Replace('__SELECTOR__', $escapedSelector).Replace('__TEXT__', $escapedText)

$located = Invoke-Cdp 'Runtime.evaluate' @{
    expression    = $locateScript
    returnByValue = $true
    awaitPromise  = $true
}
if ($located.result.exceptionDetails) {
    Write-Result "ERR:定位求值失败：$($located.result.exceptionDetails.text)"
}
$info = $located.result.result.value | ConvertFrom-Json
if (-not $info.ok) {
    Write-Result "ERR:未命中元素（selector=$Selector text=$TextMatch reason=$($info.reason)）"
}
if (-not $info.hitInside) {
    Write-Result "ERR:元素中心点被别的节点遮挡（selector=$Selector x=$($info.x) y=$($info.y)）"
}

# 2) 真实指针序列：mousedown → mouseup → click（右键则触发 contextmenu）。
$pressed = if ($Button -eq 'right') { 2 } else { 1 }
$common = @{ x = $info.x; y = $info.y; button = $Button }
Invoke-Cdp 'Input.dispatchMouseEvent' ($common + @{ type = 'mouseMoved' }) | Out-Null
# 双击要按 Chromium 的计数顺序走：先一次 clickCount=1，再一次 clickCount=2，
# 少了第一次就不会派发 dblclick，双击选词这类默认行为也就不会发生。
$counts = if ($ClickCount -gt 1) { @(1, 2) } else { @(1) }
foreach ($count in $counts) {
    Invoke-Cdp 'Input.dispatchMouseEvent' ($common + @{ type = 'mousePressed'; buttons = $pressed; clickCount = $count }) | Out-Null
    Invoke-Cdp 'Input.dispatchMouseEvent' ($common + @{ type = 'mouseReleased'; buttons = 0; clickCount = $count }) | Out-Null
}
Invoke-Cdp 'Input.dispatchMouseEvent' ($common + @{ type = 'mouseMoved'; buttons = 0 }) | Out-Null

try { $ws.Dispose() } catch { }

Write-Output "OK:button=$Button clicks=$ClickCount x=$($info.x) y=$($info.y) w=$($info.w) h=$($info.h) count=$($info.count) text=$($info.text)"
