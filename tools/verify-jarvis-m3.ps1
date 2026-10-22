<#
.SYNOPSIS
JARVIS M3 生态扩展端到端验收：外部事件入口、provider 脚本桥、正文归档与防抖。

.DESCRIPTION
在隔离的临时 Axon 根目录中启动一个独立 Daemon（独立 root、独立 IPC endpoint、独立端口），
用真实 HTTP 请求、真实 provider 脚本、真实 SQLite 与真实归档文件验证 M3-01 的入口契约。
不触碰生产 Daemon，不读写用户 ~/.axon 配置（脚本前后校验其哈希不变）。

真机闭环（真实 Jira 工单、真实仓库推送、sm-cloud 部署、钉钉通知）属于现场验收，
本脚本只在报告中标记为 manual，绝不以离线结果冒充真机证据。

.PARAMETER ProvidersRoot
provider 内容资产根目录；默认 ~/.axon/events，必须包含 jira 与 sls 两个标杆 provider。
#>
param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
    [string]$ProvidersRoot = (Join-Path $HOME ".axon/events"),
    [int]$Port = 3243,
    [switch]$SkipBuild,
    [switch]$KeepRoot,
    [string]$ReportPath = ""
)

$ErrorActionPreference = "Stop"

$PRODUCTION_GATEWAY_PORT = 3143
$GATEWAY_TOKEN_ENV = "AXON_M3_VERIFY_TOKEN"
$SOP_FILE_NAME = "sop.md"
$PARSER_FILE_NAME = "parser.py"
$ARCHIVE_DIR_NAME = "archive"
$INLINE_LIMIT_CHARS = 4000

$script:rows = New-Object System.Collections.Generic.List[object]
$script:failed = 0
$script:daemon = $null
$script:root = $null
$script:token = [guid]::NewGuid().ToString("N")

function Record([string]$Case, [bool]$Ok, [string]$Detail) {
    $status = if ($Ok) { "passed" } else { "failed" }
    if (-not $Ok) { $script:failed += 1 }
    $script:rows.Add([ordered]@{ case = $Case; status = $status; detail = $Detail })
    Write-Output ("[{0}] {1} - {2}" -f $status, $Case, $Detail)
}

function Assert-Equal($Expected, $Actual, [string]$Case, [string]$Label) {
    $ok = ($Expected -eq $Actual)
    Record $Case $ok ("{0}: 期望 {1}，实际 {2}" -f $Label, $Expected, $Actual)
    return $ok
}

function New-Directory([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { New-Item -ItemType Directory -Force -Path $Path | Out-Null }
}

function Get-FileSnapshot([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return @{ Exists = $false; Hash = "" } }
    return @{ Exists = $true; Hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash }
}

function Send-GatewayEvent([string]$Path, [string]$Body, [bool]$WithToken = $true) {
    $headers = @{}
    if ($WithToken) { $headers["Authorization"] = "Bearer $script:token" }
    $uri = "http://127.0.0.1:$Port$Path"
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Body)
    return Invoke-WebRequest -Method Post -Uri $uri -Headers $headers -ContentType "application/json; charset=utf-8" -Body $bytes -SkipHttpErrorCheck -TimeoutSec 60
}

# 制造一次被客户端中止的连接：声明正文长度后立即以 RST 关闭，模拟发送方超时或取消。
function Send-AbortedConnection {
    $client = [System.Net.Sockets.TcpClient]::new()
    $client.Connect("127.0.0.1", $Port)
    $stream = $client.GetStream()
    $head = "POST /gateway/v1/events HTTP/1.1`r`nHost: 127.0.0.1`r`nContent-Type: application/json`r`nContent-Length: 64`r`n`r`n"
    $bytes = [System.Text.Encoding]::ASCII.GetBytes($head)
    $stream.Write($bytes, 0, $bytes.Length)
    $client.Client.LingerState = [System.Net.Sockets.LingerOption]::new($true, 0)
    $client.Close()
}

function Invoke-SqliteQuery([string]$Database, [string]$Sql) {
    $helper = Join-Path $script:root "query_sqlite.py"
    $output = & python $helper $Database $Sql
    if ($LASTEXITCODE -ne 0) { throw "SQLite 查询失败：$Sql" }
    return ($output | ConvertFrom-Json)
}

function Get-ChannelCount([string]$Channel) {
    $rows = Invoke-SqliteQuery $script:database "SELECT COUNT(*) AS c FROM jarvis_interactions WHERE source_channel = '$Channel'"
    return [int]$rows[0].c
}

function Get-ChannelContent([string]$Channel) {
    $rows = Invoke-SqliteQuery $script:database "SELECT content FROM jarvis_interactions WHERE source_channel = '$Channel' ORDER BY received_at_ms DESC, interaction_id DESC"
    # 逗号运算符锁住数组形态：单元素数组会被函数输出流解包成标量，标量字符串的下标取值会退化成首字符。
    return ,@($rows | ForEach-Object { $_.content })
}


function Get-LatestEventWorkspace([string]$Channel) {
    $rows = Invoke-SqliteQuery $script:database "SELECT scope_json FROM jarvis_interactions WHERE source_channel = '$Channel' ORDER BY received_at_ms DESC, interaction_id DESC LIMIT 1"
    if ($null -eq $rows) { return "" }
    return [string](($rows[0].scope_json | ConvertFrom-Json).workspace_id)
}

# 主 turn 是异步的：HTTP 回执 202 只证明事件已耐久落库，真正的拒绝发生在之后的运行时装配阶段。
# 本套件起的是临时 root 的独立 Daemon 实例（验收夹具，不是产品机制），其中没有配置模型，
# 因此 Provider 阶段必然失败；断言只取机制事实，不看 Interaction 终态。
function Wait-Condition([scriptblock]$Probe, [int]$TimeoutSeconds = 20) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (& $Probe) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return [bool](& $Probe)
}

function New-JiraPayload([string]$Description, [string]$Summary) {
    $fields = [ordered]@{
        summary     = $Summary
        issuetype  = @{ name = "Bug" }
        status     = @{ name = "待处理" }
        priority   = @{ name = "高" }
        assignee   = @{ name = "jarvis"; displayName = "JARVIS" }
        reporter   = @{ name = "zhangsan"; displayName = "张三" }
        project    = @{ name = "AXON" }
        labels     = @("backend", "p1")
        updated    = "2026-09-20T09:30:12.000+0800"
        description = $Description
    }
    return [ordered]@{
        timestamp              = 1760000000000
        webhookEvent           = "jira:issue_updated"
        issue_event_type_name  = "issue_assigned"
        user                   = @{ name = "zhangsan"; displayName = "张三" }
        issue                  = @{ id = "10101"; key = "AXON-1"; fields = $fields }
        changelog              = @{ items = @(@{ field = "assignee"; fromString = $null; toString = "jarvis" }) }
    }
}

function New-SlsPayload([string]$FireTime) {
    return [ordered]@{
        project    = "exe-order"
        logstore   = "order-service"
        region     = "cn-hangzhou"
        alert_id   = "alert-8f21"
        alert_name = "订单导出接口 500 告警"
        status     = "firing"
        fire_time  = [int]$FireTime
        labels     = @{ service = "order-service"; level = "P1" }
        annotations = @{ title = "错误率超阈值"; desc = "5 分钟错误率 4.2%" }
        results    = @(@{ query = "status:500"; raw_results = @{ c = @("312") } })
    }
}

function Get-ErrorMessage($Response) {
    if ($null -eq $Response) { return "无响应" }
    return ([string]$Response.Content).Trim()
}

# 生成隔离运行根：独立 root / config / .env / events，与生产 Daemon 完全无关。
function Initialize-IsolatedRoot {
    $script:root = Join-Path $env:TEMP ("axon-m3-verify-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
    New-Directory $script:root
    New-Directory (Join-Path $script:root "events")
    New-Directory (Join-Path $script:root "data")
    $configPath = Join-Path $script:root "config.toml"
    $config = @"
[remote.gateway]
api_token_env = "$GATEWAY_TOKEN_ENV"
http_bind = "127.0.0.1:$Port"
websocket_bind = "127.0.0.1:$($Port + 1)"
"@
    [System.IO.File]::WriteAllText($configPath, $config, (New-Object System.Text.UTF8Encoding($false)))
    [System.IO.File]::WriteAllText((Join-Path $script:root ".env"), "$GATEWAY_TOKEN_ENV=$script:token`n", (New-Object System.Text.UTF8Encoding($false)))

    $eventsRoot = Join-Path $script:root "events"
    foreach ($provider in Get-ChildItem -LiteralPath $ProvidersRoot -Directory -ErrorAction SilentlyContinue) {
        Copy-Item -LiteralPath $provider.FullName -Destination (Join-Path $eventsRoot $provider.Name) -Recurse -Force
    }
    # 故障注入 provider：崩溃与超时必须 fail-closed。
    $crashDir = Join-Path $eventsRoot "m3crash"
    $slowDir = Join-Path $eventsRoot "m3slow"
    New-Directory $crashDir
    New-Directory $slowDir
    [System.IO.File]::WriteAllText((Join-Path $crashDir $PARSER_FILE_NAME), "import sys`nsys.stderr.write('m3-crash')`nsys.exit(3)`n", (New-Object System.Text.UTF8Encoding($false)))
    [System.IO.File]::WriteAllText((Join-Path $slowDir $PARSER_FILE_NAME), "import json, sys, time`ntime.sleep(5)`nprint(json.dumps({'event_id': 'm3slow:1', 'title': 'slow', 'content': 'slow'}))`n", (New-Object System.Text.UTF8Encoding($false)))

    $helper = Join-Path $script:root "query_sqlite.py"
    $helperSource = @"
import json, sqlite3, sys
# Windows 上 Python 默认 stdio 编码不是 UTF-8，会让读回的中文变成乱码；必须显式锁死。
for stream in (sys.stdout,):
    reconfigure = getattr(stream, "reconfigure", None)
    if reconfigure is not None:
        reconfigure(encoding="utf-8")
uri = "file:%s?mode=ro" % sys.argv[1].replace("\\", "/")
connection = sqlite3.connect(uri, uri=True)
connection.row_factory = sqlite3.Row
rows = [dict(row) for row in connection.execute(sys.argv[2])]
print(json.dumps(rows, ensure_ascii=False))
"@
    [System.IO.File]::WriteAllText($helper, $helperSource, (New-Object System.Text.UTF8Encoding($false)))
    $script:configPath = $configPath
    $script:database = Join-Path $script:root "data/runtime.sqlite3"
    $script:archiveRoot = Join-Path $script:root "data/$ARCHIVE_DIR_NAME"
}

function Start-IsolatedDaemon {
    $binary = Join-Path $WorkspaceRoot "target/release/axon.exe"
    $stdout = Join-Path $script:root "daemon.out.log"
    $stderr = Join-Path $script:root "daemon.err.log"
    $arguments = @("daemon", "run", "--root", $script:root, "--config-path", $script:configPath, "--endpoint", "axon-m3-verify")
    $script:daemon = Start-Process -FilePath $binary -ArgumentList $arguments -WorkingDirectory $WorkspaceRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $script:stdoutLog = $stdout
    $script:stderrLog = $stderr
}

function Wait-GatewayReady([int]$TimeoutSeconds = 90) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ($script:daemon.HasExited) { throw "隔离 Daemon 提前退出，退出码 $($script:daemon.ExitCode)" }
        try {
            $response = Send-GatewayEvent "/gateway/v1/events" "{}" $false
            if ($response.StatusCode -eq 401) { return $true }
        } catch {
            Start-Sleep -Milliseconds 500
        }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Stop-IsolatedDaemon {
    if ($null -ne $script:daemon -and -not $script:daemon.HasExited) {
        $script:daemon.Kill()
        $script:daemon.WaitForExit(15000) | Out-Null
    }
}

$guardedFiles = @((Join-Path $HOME ".axon/config.toml"), (Join-Path $HOME ".axon/.env"))
$snapshotsBefore = @{}
foreach ($path in $guardedFiles) { $snapshotsBefore[$path] = Get-FileSnapshot $path }

$warning = $null
try {
    Set-Location $WorkspaceRoot
    if ($Port -eq $PRODUCTION_GATEWAY_PORT) { throw "验收端口不能占用生产 Gateway 端口 $PRODUCTION_GATEWAY_PORT" }

    if (-not $SkipBuild) {
        Write-Output "==> 构建 release 二进制"
        & cargo build --release -p axon-cli
        if ($LASTEXITCODE -ne 0) { throw "release 构建失败" }
    }

    Write-Output "==> provider 内容资产检查（$ProvidersRoot）"
    foreach ($provider in @("jira", "sls")) {
        $parser = Join-Path $ProvidersRoot "$provider/$PARSER_FILE_NAME"
        $sop = Join-Path $ProvidersRoot "$provider/$SOP_FILE_NAME"
        Record "asset_$($provider)_parser" (Test-Path -LiteralPath $parser) $parser
        Record "asset_$($provider)_sop" (Test-Path -LiteralPath $sop) $sop
    }

    Initialize-IsolatedRoot
    Write-Output "==> 隔离运行根：$script:root"
    Start-IsolatedDaemon
    $ready = Wait-GatewayReady
    Record "gateway_ready" $ready "独立 Daemon 监听 127.0.0.1:$Port 并在缺 token 时返回 401"

    $header = @{ Authorization = "Bearer $script:token" }

    # S1：无 token 必须 401（鉴权早于解析与落库）
    $response = Send-GatewayEvent "/gateway/v1/events" "{ not json" $false
    Assert-Equal 401 $response.StatusCode "no_token_rejected" "缺 token 状态码"

    # S2：Jira 指派事件（原生直通 -> parser.py 归一化 -> durable Interaction）
    $assigned = (New-JiraPayload "压测时 /order/export 返回 500，栈里有 NPE。" "订单导出接口偶发 500" | ConvertTo-Json -Depth 12 -Compress)
    $response = Send-GatewayEvent "/gateway/v1/events/jira/raw" $assigned $true
    Assert-Equal 202 $response.StatusCode "jira_assignment_accepted" "指派事件状态码"
    $receipt = $response.Content | ConvertFrom-Json
    Assert-Equal $true $receipt.accepted "jira_assignment_accepted" "accepted"
    $count = Get-ChannelCount "jira"
    Assert-Equal 1 $count "jira_assignment_persisted" "jira Interaction 行数"
    $content = (Get-ChannelContent "jira")[0]
    Record "jira_assignment_brief_title" ($content -like "*标题：*订单导出接口偶发 500*") "简报含标题"
    Record "jira_assignment_brief_sop" ($content -like "*流程文档：*jira*sop.md*") "简报含 SOP 路径"
    Record "jira_assignment_brief_event_id" ($content -like "*事件标识：jira:AXON-1:assigned:*") "简报含事件标识"
    Record "jira_assignment_brief_utf8" ($content -like "*已指派*") "中文未损坏"

    # S2b：事件必须真被主 turn 接单。判据用两条机制事实：事件绑定的 Workspace 已注册，
    # 且主 turn 确实为该 Workspace 打开了运行时会话——只看 HTTP 回执会漏检这类装配期拒绝。
    $eventWorkspace = Get-LatestEventWorkspace "jira"
    $workspaceRegistered = Wait-Condition {
        $rows = Invoke-SqliteQuery $script:database "SELECT COUNT(*) AS c FROM jarvis_workspaces WHERE workspace_id = '$eventWorkspace'"
        return ([int]$rows[0].c -ge 1)
    }
    Record "external_event_workspace_registered" $workspaceRegistered "外部事件绑定的 Workspace 已注册（$eventWorkspace）"
    $runtimeOpened = Wait-Condition {
        $rows = Invoke-SqliteQuery $script:database "SELECT COUNT(*) AS c FROM session_runtimes WHERE session_id = 'session-lane-$eventWorkspace'"
        return ([int]$rows[0].c -ge 1)
    }
    Record "external_event_opens_runtime_session" $runtimeOpened "主 turn 已为事件打开运行时（session-lane-$eventWorkspace）"

    # S3：同一 event_id 重复投递必须幂等（不新增 Interaction）
    $response = Send-GatewayEvent "/gateway/v1/events/jira/raw" $assigned $true
    $receipt = $response.Content | ConvertFrom-Json
    Assert-Equal $true $receipt.deduplicated "jira_duplicate_deduplicated" "重复投递 deduplicated"
    Assert-Equal 1 (Get-ChannelCount "jira") "jira_duplicate_no_new_row" "重复投递后 jira 行数"

    # S4：Jira 评论事件产生新 Interaction
    $commentPayload = New-JiraPayload "压测时 /order/export 返回 500。" "订单导出接口偶发 500"
    $commentPayload["issue_event_type_name"] = "issue_commented"
    $commentPayload["comment"] = [ordered]@{ id = "10888"; author = @{ name = "lisi"; displayName = "李四" }; created = "2026-09-20T10:05:00.000+0800"; body = "复现步骤：并发 50 时必现。" }
    $commentPayload["issue"]["fields"]["updated"] = "2026-09-20T10:05:00.000+0800"
    $response = Send-GatewayEvent "/gateway/v1/events/jira/raw" ($commentPayload | ConvertTo-Json -Depth 12 -Compress) $true
    Assert-Equal 202 $response.StatusCode "jira_comment_accepted" "评论事件状态码"
    Assert-Equal 2 (Get-ChannelCount "jira") "jira_comment_persisted" "评论事件后 jira 行数"
    $contents = Get-ChannelContent "jira"
    Record "jira_comment_brief" (-not [string]::IsNullOrWhiteSpace(($contents | Where-Object { $_ -like "*复现步骤*" }))) "评论正文进入简报"

    # S5：超长正文必须归档，Interaction 只保留简报与归档路径。
    # provider 脚本对单字段做有界截断（描述 2000 / 评论 1200），超长正文由多字段叠加构成；
    # 特征串放在归一化正文末尾，用于证明归档保存的是全文而非前 4000 字符。
    # 换用另一张工单（AXON-2）投递，避免落进 AXON-1 指派事件的防抖窗口。
    $longMarker = "M3-ARCHIVE-MARKER"
    $oversizedPayload = New-JiraPayload "" "超长正文工单"
    $oversizedPayload["issue_event_type_name"] = "issue_updated"
    $oversizedPayload["issue"]["key"] = "AXON-2"
    $changeFields = @("description", "summary", "labels", "components", "fixVersions", "priority")
    $oversizedItems = @(0..($changeFields.Count - 1) | ForEach-Object {
        [ordered]@{ field = $changeFields[$_]; fromString = ("变更前填充" * 90); toString = ("变更后填充" * 90) }
    })
    $oversizedItems[-1]["toString"] = $oversizedItems[-1]["toString"] + $longMarker
    $oversizedPayload["changelog"] = [ordered]@{ items = $oversizedItems }
    $oversized = ($oversizedPayload | ConvertTo-Json -Depth 12 -Compress)
    $response = Send-GatewayEvent "/gateway/v1/events/jira/raw" $oversized $true
    Assert-Equal 202 $response.StatusCode "oversized_accepted" "超长正文事件状态码"
    $contents = Get-ChannelContent "jira"
    $archivedBrief = $contents | Where-Object { $_ -like "*正文归档：*" } | Select-Object -First 1
    Record "oversized_brief_points_to_archive" (-not [string]::IsNullOrWhiteSpace($archivedBrief)) "简报给出归档路径"
    $archivePath = $null
    if ($archivedBrief) {
        $line = ($archivedBrief -split "`n" | Where-Object { $_ -like "正文归档：*" } | Select-Object -First 1)
        $archivePath = ($line -replace "^正文归档：", "").Split([char]0xFF08)[0].Trim()
    }
    $archiveOk = ($null -ne $archivePath) -and (Test-Path -LiteralPath $archivePath)
    Record "oversized_archive_file_exists" $archiveOk ("归档文件：{0}" -f $archivePath)
    if ($archiveOk) {
        $archived = Get-Content -LiteralPath $archivePath -Raw -Encoding utf8
        Record "oversized_archive_is_complete" ($archived -like "*$longMarker*") "归档正文含原始尾部特征串"
        Record "oversized_brief_is_bounded" ($archivedBrief.Length -lt $INLINE_LIMIT_CHARS) ("简报长度 {0} < {1}" -f $archivedBrief.Length, $INLINE_LIMIT_CHARS)
    }
    $archiveFiles = @(Get-ChildItem -LiteralPath $script:archiveRoot -File -Recurse -ErrorAction SilentlyContinue)
    Record "archive_directory_used" ($archiveFiles.Count -ge 1) ("归档目录文件数 {0}" -f $archiveFiles.Count)

    # S6：SLS 告警连发必须被防抖合并为一条 Interaction
    for ($index = 1; $index -le 3; $index++) {
        $payload = (New-SlsPayload (1760000300 + $index) | ConvertTo-Json -Depth 8 -Compress)
        $response = Send-GatewayEvent "/gateway/v1/events/sls/raw" $payload $true
        Assert-Equal 202 $response.StatusCode "sls_burst_item_$index" "连发第 $index 条状态码"
        if ($index -gt 1) {
            $receipt = $response.Content | ConvertFrom-Json
            Assert-Equal $true $receipt.deduplicated "sls_burst_item_$index" "连发第 $index 条 deduplicated"
        }
    }
    Assert-Equal 1 (Get-ChannelCount "sls") "sls_burst_debounced" "防抖后 sls Interaction 行数"

    # S7：未注册 provider 必须 404 且不落库
    $response = Send-GatewayEvent "/gateway/v1/events/unknownp/raw" $assigned $true
    Assert-Equal 404 $response.StatusCode "unregistered_provider_rejected" "未注册 provider 状态码"
    Assert-Equal 0 (Get-ChannelCount "unknownp") "unregistered_provider_no_row" "未注册 provider Interaction 行数"

    # S8：脚本崩溃与超时必须 fail-closed（不落库脏数据）
    $response = Send-GatewayEvent "/gateway/v1/events/m3crash/raw" $assigned $true
    Assert-Equal 502 $response.StatusCode "parser_crash_fails_closed" "脚本崩溃状态码"
    Assert-Equal 0 (Get-ChannelCount "m3crash") "parser_crash_no_row" "脚本崩溃 Interaction 行数"

    $response = Send-GatewayEvent "/gateway/v1/events/m3slow/raw" $assigned $true
    Assert-Equal 504 $response.StatusCode "parser_timeout_fails_closed" "脚本超时状态码"
    Assert-Equal 0 (Get-ChannelCount "m3slow") "parser_timeout_no_row" "脚本超时 Interaction 行数"

    # S9：既有通用入口语义不变
    $legacy = [ordered]@{
        sourceChannel    = "m3legacy"
        eventId          = "legacy-1"
        observedAtMs     = [long]((Get-Date).ToUniversalTime() - [datetime]::UnixEpoch).TotalMilliseconds
        expiresAtMs      = [long]((Get-Date).ToUniversalTime() - [datetime]::UnixEpoch).TotalMilliseconds + 600000
        content          = "通用入口短正文"
        replyChannel     = "m3legacy"
        replyDestination  = "m3legacy:legacy-1"
    }
    $response = Send-GatewayEvent "/gateway/v1/events" ($legacy | ConvertTo-Json -Compress) $true
    Assert-Equal 202 $response.StatusCode "generic_route_accepted" "通用入口状态码"
    $legacyContent = (Get-ChannelContent "m3legacy")[0]
    Assert-Equal "通用入口短正文" $legacyContent "generic_route_content_verbatim" "通用入口短正文原样落库"

    # S10：被中止的连接不得杀死入口——发送方超时/取消是 webhook 的常态失败，
    # 监听循环一旦因连接级错误退出，Daemon 会「进程在跑但入口已死」。
    for ($attempt = 0; $attempt -lt 3; $attempt++) { Send-AbortedConnection }
    $response = Send-GatewayEvent "/gateway/v1/events" "{}" $false
    Assert-Equal 401 $response.StatusCode "aborted_connection_keeps_ingress" "中止连接后入口仍可用（缺 token 返回 401）"

    # S11：URL 查询串 token（Jira Server WebHooks 无法配置 header，只能把凭据写在 URL 上）
    $queryEnvelope = [ordered]@{
        sourceChannel    = "m3query"
        eventId          = "query-1"
        observedAtMs     = [long]((Get-Date).ToUniversalTime() - [datetime]::UnixEpoch).TotalMilliseconds
        expiresAtMs      = [long]((Get-Date).ToUniversalTime() - [datetime]::UnixEpoch).TotalMilliseconds + 600000
        content          = "查询串 token 正文"
        replyChannel     = "m3query"
        replyDestination = "m3query:query-1"
    }
    $response = Send-GatewayEvent "/gateway/v1/events?api_token=$script:token" ($queryEnvelope | ConvertTo-Json -Compress) $false
    Assert-Equal 202 $response.StatusCode "query_token_accepted" "URL 查询串 token 状态码"
    $jiraBefore = Get-ChannelCount "jira"
    $forbiddenPayload = New-JiraPayload "越权投递" "越权工单"
    $forbiddenPayload["issue"]["key"] = "AXON-3"
    $response = Send-GatewayEvent "/gateway/v1/events/jira/raw?api_token=wrong-token" ($forbiddenPayload | ConvertTo-Json -Depth 12 -Compress) $false
    Assert-Equal 401 $response.StatusCode "wrong_query_token_rejected" "错误查询串 token 状态码"
    Assert-Equal $jiraBefore (Get-ChannelCount "jira") "wrong_query_token_no_row" "错误查询串 token 未落库"
} catch {
    $warning = $_.Exception.Message
    Record "verify_exception" $false $warning
    if ($null -ne $script:stderrLog -and (Test-Path -LiteralPath $script:stderrLog)) {
        Write-Output "--- daemon stderr 尾部 ---"
        Get-Content -LiteralPath $script:stderrLog -Tail 30
    }
} finally {
    Stop-IsolatedDaemon
    foreach ($path in $guardedFiles) {
        $after = Get-FileSnapshot $path
        $before = $snapshotsBefore[$path]
        $unchanged = ($before.Exists -eq $after.Exists) -and ($before.Hash -eq $after.Hash)
        Record "user_config_untouched" $unchanged $path
    }
    if (-not $KeepRoot -and $null -ne $script:root -and (Test-Path -LiteralPath $script:root)) {
        Remove-Item -LiteralPath $script:root -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$manual = @(
    [ordered]@{ case = "site_jira_assignment_closed_loop"; status = "manual"; detail = "真实 Jira 指派 -> 取单/拉仓库/修复/推送/构建部署/回写/钉钉通知" },
    [ordered]@{ case = "site_jira_comment_closed_loop"; status = "manual"; detail = "真实 Jira 评论 -> 增量处理并回写" },
    [ordered]@{ case = "site_high_risk_approval_gate"; status = "manual"; detail = "审批未授予时高风险动作被拦截且不重试" },
    [ordered]@{ case = "site_external_outage_does_not_block_local"; status = "manual"; detail = "Jira/SLS/构建平台故障时不阻塞本地 Task" }
)

$report = [ordered]@{
    suite         = "jarvis-m3"
    status        = if ($script:failed -eq 0) { "passed" } else { "failed" }
    failedChecks  = $script:failed
    totalChecks   = $script:rows.Count
    isolatedRoot  = $script:root
    gatewayPort   = $Port
    providerRoot  = $ProvidersRoot
    finishedAt    = [DateTimeOffset]::UtcNow.ToString("o")
    checks        = $script:rows
    manualChecks  = $manual
} | ConvertTo-Json -Depth 8

Write-Output $report
if ($ReportPath) { Set-Content -LiteralPath $ReportPath -Value $report -Encoding utf8 }
Write-Output "M3 入口验收：$($script:rows.Count) 项检查，失败 $($script:failed) 项。"
Write-Output "说明：真实 Jira 工单闭环、真实推送与部署、钉钉通知/审批属于现场验收，见报告中 manualChecks。"
if ($script:failed -gt 0) { exit 1 }