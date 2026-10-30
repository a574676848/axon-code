# Axon HTTP 接口

本文件只描述已安装 Axon 在运行态暴露的 HTTP 接口：监听地址、鉴权、路由、报文体与错误码。所有取值以当前实例的实际响应为准：先 `axon config get <key>` 读取监听与凭据名，再用真实请求核对状态码与响应体。

## 两个 HTTP 面

| 面 | 监听地址 | 由谁提供 | 方法 |
|---|---|---|---|
| Gateway HTTP | `remote.gateway.http_bind`（`axon config get remote.gateway.http_bind`） | Daemon 常驻，随 Daemon 启动 | `GET` / `POST` / `DELETE` |
| headless server | `axon serve --host <host> --port <port>`（默认 `127.0.0.1:3000`） | 手动启动的前台进程 | 仅 `GET` |

- 两个面互相独立：Gateway 属于 Daemon，Daemon 停止时该面不可达；`axon serve` 是独立进程，不依赖 Daemon。
- Gateway 的 WebSocket 面在 `remote.gateway.websocket_bind`，与 HTTP 面是不同端口。

## 鉴权

Gateway 统一使用 API token，两条通道任一命中即通过：

```text
Authorization: Bearer <token>
<path>?api_token=<token>      # 供无法配置 HTTP header 的发送方使用
```

- token 的环境变量名由 `remote.gateway.api_token_env` 指定（`axon config get remote.gateway.api_token_env`）；Daemon 先读进程环境，再读配置相邻的 env 文件（`axon config env-path`）。
- **未配置 token 时 Gateway 的全部路由一律返回 401**（fail-closed）；发送方与 Daemon 必须使用同一个 token。
- 查询串通道不做百分号解码：token 必须由 URL 安全字符构成，否则视为未提供。
- `axon serve` 用 `--auth-token-file <path>` 指定 token 文件，文件内容为 token 原文，长度 16 到 4096 字节；绑定非 loopback 地址时必须提供该文件；`GET /v1/health` 免鉴权。
- 只读取认证状态，不打印、不记录 token 原文。

## Gateway 路由

| 方法与路径 | 用途 |
|---|---|
| `POST /gateway/v1/events` | 通用外部事件，`sourceChannel` 由报文体指定，不调用 provider 解析脚本 |
| `POST /gateway/v1/events/<provider>` | 已完成归一化的 provider 信封，报文体即解析脚本的输出契约 |
| `POST /gateway/v1/events/<provider>/raw` | provider 原生报文，交给该 provider 的解析脚本归一化 |
| `GET /gateway/v1/interactions/<interactionId>` | 查询指定交互的处理结果 |
| `/api/v1/...` | 扩展路由族，见下节 |

`POST` 的硬性要求：

- 必须带 `Content-Type: application/json`，否则 415；
- 必须带 `Content-Length`，否则 411；
- 请求头上限 16 KiB，报文体上限 300 MiB；大内容不在传输层裁剪，而是在进入事件流之前按预算落归档（见 `docs/tool-schema.md`）；
- 受理成功返回 `202` 与回执 JSON，字段为 `interactionId`、`accepted`、`deduplicated`。

### 通用事件报文体

字段为 camelCase，未知字段被拒（`400`）。

```json
{
  "sourceChannel": "jira",
  "eventId": "AX-1024",
  "workspaceId": "",
  "observedAtMs": 1730000000000,
  "expiresAtMs": 1730086400000,
  "content": "事件正文",
  "replyChannel": "jira",
  "replyDestination": "AX-1024",
  "localFiles": []
}
```

- `sourceChannel`、`eventId`、`observedAtMs`、`expiresAtMs`、`content`、`replyChannel`、`replyDestination` 必填；时间戳为毫秒。
- `workspaceId` 省略或仅空白时由服务端解析默认 Workspace；显式给出时必须已注册，否则整条事件被拒。
- `localFiles` 为本地附件的绝对路径数组，可省略。
- `sourceChannel` 为 `desktop` 时提交方同时给出显式 `workspaceId`。

### provider 路由的前置条件

`/gateway/v1/events/<provider>` 与 `/gateway/v1/events/<provider>/raw` 要求该 provider 已在运行根的 `events/<provider>/` 目录中配置（目录名即 provider 名，其中解析脚本 `parser.py` 必填、`sop.md` 可选）。未配置时返回 `404`。

### 扩展路由族（`/api/v1`，全部需要鉴权）

工作台与车道：

| 方法与路径 | 用途 |
|---|---|
| `GET /api/v1/workspaces` | 工作区注册表 |
| `POST /api/v1/workspaces` | 注册工作区 |
| `GET /api/v1/lanes` | 车道视图，可用 `?workspace=<id>` 过滤 |
| `POST /api/v1/lanes/identity` | 由报文体推导车道标识 |
| `GET /api/v1/lanes/<laneId>/subagents` | 该车道的子代理清单 |
| `POST /api/v1/lanes/<laneId>/interrupt` | 中断该车道的运行 |
| `GET /api/v1/lanes/<laneId>/interactions` | 该车道的交互记录；`?limit=<n>` 限制条数，`?before_ms=<毫秒>&before_id=<交互标识>` 向前翻页（取该条之前的一页，两个参数必须成对给出） |
| `GET /api/v1/interactions/<interactionId>/events` | 该交互的历史帧页；`?after=<序号>` 从该序号之后续读（缺省从头），`?limit=<n>` 限制本页帧数。返回 `events`、`nextAfter`、`hasMore`、`tailSequence`、`tailCursor`：`hasMore` 为真时按 `nextAfter` 继续翻页，读完用 `tailCursor` 订阅 WebSocket 即可只收增量 |
| `GET /api/v1/lanes/<laneId>/jobs` | 该车道的作业清单 |
| `POST /api/v1/lanes/<laneId>/jobs/<jobId>/stop` | 停止指定作业 |
| `POST /api/v1/lanes/<laneId>/subagents/<agent>/messages` | 给指定子代理追加消息 |
| `GET /api/v1/lanes/<laneId>/questions` | 该车道的提问记录，可用 `?state=<pending\|suspended\|answered\|cancelled\|expired>` 过滤 |
| `POST /api/v1/lanes/<laneId>/questions/<callId>/answer` | 提交待决提问的结构化回答，先到先得；已结算返回 409 |

`<agent>` 是单个路径段，其中的 `/` 与 `:` 必须先做百分号转义；转义非法按未命中处理。

目标看板（只读；桌面侧栏「目标看板」经此读取，不写账本）：

| 方法与路径 | 用途 |
|---|---|
| `GET /api/v1/goals` | 可见目标列表。`?limit=<1-256>`，缺省 256，越界返回 400 `invalid_query`；`?workspaceId=<id>` 只返回该工作区的目标。返回 `goals`、`returned`、`truncated`。每个目标带 `goalId`、`title`、`status`、`revision`、`objectivePreview`（按 160 字符截断）、`stepCount`、`nextStep`、`waitingReason`、`priority`、`deadlineMs`、`scopeKind`（`workspace` 或 `global`）、`workspaceId`、`workspaceName`、`workspacePath`、`createdAtMs`、`updatedAtMs`、`lastActivityAtMs`、`latestExecution`、`latestResult`。`lastActivityAtMs` 取更新时间、最近执行更新时间与最近结果时间三者最大；`truncated` 为真表示本次读取达到上限且仍有更多目标 |
| `GET /api/v1/goals/<goalId>` | 单个目标详情。在列表同构的 `goal` 之上追加 `objective`（最多 20000 字符）、`steps`、`acceptance`（各最多 64 条、单条最多 2000 字符）、`cancellationReason`、`objectiveTruncated`、`listsTruncated`、`recentExecutions`（最多 8 条，按 `createdAtMs` 降序）与 `latestResult`。目标不存在或不可见返回 404 `goal_not_found` |

目标执行会话的逐字稿不另立路由：走 WebSocket 的会话级订阅，见 `gateway-websocket.md`。

人格状态（只读；桌面设置 › 人设 的状态面板经此读取）：

| 方法与路径 | 用途 |
|---|---|
| `GET /api/v1/persona/summary` | 返回 `soul`（内核解析结论）、`owner`、`state`（无状态时为 null）、`transitions`（最近 10 条）、`style`（已发布习得风格）与 `database`（库状态与体积）。人格库不存在时如实报告未创建，不创建文件 |
| `GET /api/v1/persona/portrait/<file>.png` | 读取 `~/.axon/persona/` 下的形象资产，返回 `{"file","mime","base64"}`；只接受该目录内的 `.png`，越界或缺失一律 404 |

MCP 运行态（报文体为 `{"cwd": "<工作区路径>"}`，`cwd` 缺省时用 daemon 声明的工作区）：

| 方法与路径 | 用途 |
|---|---|
| `POST /api/v1/mcp/status` | 当前工作区已加载的 MCP 服务器：`servers`（每台带 `serverId`、`toolCount`、`tools`、`failed`、`failure`、`disposition`）、`totalCount`、`loadedCount`、`failedCount`；读不到结论时带 `error` |
| `POST /api/v1/mcp/refresh` | 清 MCP 健康负缓存并强制重建启动快照，返回同一份载荷外加 `refreshed` 与 `healthCacheCleared` |

两台入口的加载结论都来自 daemon 的启动快照（与 turn 用的是同一份），不是另起一次探测；`refresh` 才会真的重连一遍 MCP 服务器。

Cron：

| 方法与路径 | 用途 |
|---|---|
| `GET /api/v1/crons` | 列表，支持查询参数 |
| `POST /api/v1/crons` | 创建 |
| `GET /api/v1/crons/config` | 读取 Cron 配置，含 `defaultNotificationChatId`（用户未指明目标时的默认 `wake` 建议值） |
| `GET /api/v1/crons/<id>` | 读取单个 Cron |
| `POST /api/v1/crons/<id>` | 更新 |
| `DELETE /api/v1/crons/<id>` | 移除 |
| `GET /api/v1/crons/<id>/runs` | 运行历史 |
| `POST /api/v1/crons/<id>/pause`、`/resume`、`/run` | 暂停、恢复、立即运行 |

可观测性：

| 方法与路径 | 用途 |
|---|---|
| `GET /api/v1/observability/overview` | 看板概览，支持 `period`、`provider`、`model`、`source` |
| `GET /api/v1/observability/realtime` | 实时监控滑动窗口 |
| `GET /api/v1/observability/distribution` | 用量分布与排行 |
| `GET /api/v1/observability/diagnostics` | 延迟分位数与效率诊断 |
| `GET /api/v1/observability/audit/logs` | 审计日志检索 |
| `GET /api/v1/observability/audit/export` | 审计日志导出（CSV 或 JSON） |
| `GET /api/v1/observability/pricing/rules` | 费率规则列表 |
| `POST /api/v1/observability/pricing/rules` | 新增费率规则（规则 ID 由服务端生成） |
| `DELETE /api/v1/observability/pricing/rules/<id>` | 删除费率规则 |
| `POST /api/v1/observability/pricing/sync` | 同步官方模型价格；只收录与本机配置同名供应商的条目（跳过全零条目、按匹配式与单价去重，写入规则优先级为 -1，手工规则优先）；返回 `outcome`（`success`、`debounced`、`rate_limited`、`fallback`）与 `detail` |
| `POST /api/v1/observability/pricing/recalculate` | 重新核算历史用量 |
| `GET /api/v1/observability/pricing/auto-sync` | 每日自动同步任务的状态（`enabled`、`status`、`lastRunAtMs`、`nextRunAtMs`） |
| `POST /api/v1/observability/pricing/auto-sync` | 报文 `{"enabled":true|false}`：创建或启用、暂停每日同步任务 |
| `GET /api/v1/observability/auth/probe` | 固定返回 401，用于确认鉴权链路已生效 |

### 错误码

错误体有两种形态：网关自身拒绝返回 `{"error":"<code>"}`；鉴权失败与可观测性等扩展路由族返回统一错误模型 `{"error":{"code":"...","message":"...","details":...}}`，工作台族的 `error` 为消息字符串。

| 状态 | `error` | 含义与处置 |
|---|---|---|
| 401 | `UNAUTHORIZED` | token 缺失或不匹配，或实例未配置 token；先核对 token 名与取值 |
| 400 | `invalid_json`、`invalid_event` | 报文不合法；核对 JSON、字段拼写与取值 |
| 403 | `event_rejected` | 事件被入口策略拒绝，事件未入账 |
| 404 | `not_found`、`provider_not_configured`、`interaction_not_found` | 路由未命中，或 provider 未在 `events/<provider>/` 配置 |
| 405 | `method_not_allowed` | 方法不被该路由接受 |
| 431 | `headers_too_large` | 请求头总长超过 16 KiB |
| 411、415 | `content_length_required`、`content_type_required` | 补 `Content-Length` 或改为 `application/json` |
| 413 | `body_too_large` | 报文体超过上限 |
| 429 | `gateway_busy` | 连接许可或阻塞池排队预算已满，稍后重试 |
| 502、504 | `event_script_failed`、`event_script_timeout` | provider 解析脚本失败或超时，事件未入账 |
| 503 | `gateway_unavailable`、`event_prepare_failed` | 生命周期或准备失败，稍后重试 |

目标看板面另有三个错误码，取统一错误模型 `{"error":{"code":"...","message":"...","details":null}}`：

| 状态 | `code` | 含义与处置 |
|---|---|---|
| 400 | `invalid_query` | `limit` 不在 1 到 256 之间，或 `workspaceId` 不是合法的 Workspace 标识；按提示改正查询串 |
| 404 | `goal_not_found` | 目标不存在或对当前可见范围不可见；不要按「后台版本过旧」处置 |
| 500 | `goal_query_failed` | 账本读取失败；把 `agent.log_level` 调到 `info`，再按 `path=/api/v1/goals` 与 `status=500` 检索 `data/logs/axon-<日期>.log` 的访问日志行。错误码只在响应体里，日志行不带 |

无错误码的 404 仍表示路由不存在。桌面端对这两种 404 给不同结论：有 `goal_not_found` 时报「目标不存在或已被清理」，没有错误码时报「网关不含该接口，后台服务版本可能过旧」。

### 并发与背压

- 传输由 axum（hyper + tokio）承载：HTTP 解析、连接管理与优雅停机都在框架内，网关自身不再持有固定数量的工作线程。
- 同步业务（运行态存储读写、provider 解析脚本、事件落库）在阻塞池执行，IO 线程始终可服务；单个慢请求不会让其后的请求排队。
- 两级准入先到先得：内核连接许可（`GatewayConnectionLimiter`，默认 256）与阻塞池并发闸门（64）。阻塞池满载时请求最多排队 5 秒，仍拿不到许可返回 `429 gateway_busy`。
- 慢发请求头不再由服务端主动断开（因此不再返回 `408 request_timeout`），连接在读头阶段只占用极少的运行时资源。

### 访问日志

- 每个请求写一条 `gateway-http method=<方法> path=<路径> status=<状态码> elapsed_ms=<耗时>` 的日志；状态码 >= 400 或耗时 >= 500ms 记为 `WARNING`，其余记为 `INFO`。
- 可见级别由 `agent.log_level` 决定（`axon config get agent.log_level`）：要看全部访问日志设为 `info`，只想看慢请求与失败设为 `warning`；默认的 `error` 下这两类都不会落盘。
- 日志写在运行根的 `data/logs/axon-<日期>.log`（运行根即 `axon config path` 所在目录）；排查偶发卡顿时先看 `elapsed_ms`，再对照同分钟的其它错误行。

Gateway 面只在 Daemon 运行时可达：先用 `axon daemon status` 确认 `installed` 与 `running`，再发请求。扩展路由族的未命中子路径返回该族自己的 404 错误体，不要以统一的 `not_found` 判断。

## headless server 路由（`axon serve`）

| 方法与路径 | 用途 |
|---|---|
| `GET /v1/health` | 免鉴权；返回 `status`、`service`、`version`、`authRequired` 与 `capabilities` |
| `GET /v1/ready` | 已加载的 provider 与模型数量 |
| `GET /v1/models` | 已启用模型清单 |
| `GET /v1/workspaces` | 工作区视图 |
| `GET /v1/lanes` | 车道视图，可用 `?workspace=<id>` 过滤 |
| `GET /v1/sessions` | 会话摘要，可用 `?limit=<n>` 限制条数 |

非 `GET` 方法返回 405；鉴权头格式为 `Authorization: Bearer <token>`，与 `--auth-token-file` 中的原文一致。

`GET /v1/health` 的 `capabilities.protocolVersion` 是工作台协议版本，当前为 6；每新增一个扩展路由族或改动其字段语义时递增。它是服务端能力声明，桌面端不做客户端协商。

## 调用脚本

与 `SKILL.md` 同级的 `scripts/gateway-http.ps1` 封装上述调用：解析监听地址与 token、拼装报文体、输出状态码与响应体。它不打印 token 原文。

```powershell
# 只读检查：看板概览（默认走 Gateway 面，基址由配置推导）
pwsh -File scripts/gateway-http.ps1 -Action overview -Query "period=7d"

# 车道与工作区
pwsh -File scripts/gateway-http.ps1 -Action lanes -Query "workspace=<workspace-id>"
pwsh -File scripts/gateway-http.ps1 -Action workspaces

# 提交一条通用外部事件
pwsh -File scripts/gateway-http.ps1 -Action event -SourceChannel jira -EventId AX-1024 `
  -Content "事件正文" -ReplyChannel jira -ReplyDestination AX-1024 -ExpiresInMinutes 1440

# 查询交互结果
pwsh -File scripts/gateway-http.ps1 -Action interaction -InteractionId <interaction-id>

# 任意路由
pwsh -File scripts/gateway-http.ps1 -Action raw -Method GET -Path "/api/v1/crons"

# headless server：需要显式给出基址与 token 文件
pwsh -File scripts/gateway-http.ps1 -Surface serve -BaseUrl "http://127.0.0.1:3000" `
  -TokenFile <token-file> -Action health
```

判定口径：输出以 `OK:<状态码>` 或 `ERR:<状态码>` 开头，并带上目标 URL；退出码 0 表示 2xx，1 表示其他情况。核验时必须同时看前缀与退出码。

## curl 等价写法

```bash
BIND="$(axon config get remote.gateway.http_bind | awk -F'= ' '{print $2}')"
BASE="http://${BIND/0.0.0.0/127.0.0.1}"
TOKEN_ENV="$(axon config get remote.gateway.api_token_env | awk -F'= ' '{print $2}')"

curl -sS -H "Authorization: Bearer ${!TOKEN_ENV}" "$BASE/api/v1/observability/overview?period=7d"
```

## 核验与处置原则

- 只读优先：先 `GET`，确认目标与副作用后再发 `POST`、`DELETE`。
- 回执以状态码加响应体为准；不把自然语言结论当作调用成功。
- 出现 401 时依次核对 token 名、token 取值来源与 Daemon 是否已重新加载 env 文件。
- 出现 404 且路径含 provider 名时，先确认 `events/<provider>/` 与解析脚本是否存在。
- 修改费率、重算用量、中断车道、停止作业、投递子代理消息都属于状态变更，须先取得用户授权。
