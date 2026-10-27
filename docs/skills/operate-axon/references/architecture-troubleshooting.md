# Axon 运行时诊断

只对已安装 Axon 实例进行诊断。先收集只读信息，不要用删除配置、会话或 Memory 的方式掩盖问题。

## 基础检查

```text
axon --version
axon config path
axon config env-path
axon config validate
axon doctor --check preflight
axon daemon status
axon config get remote.gateway.http_bind
axon config get remote.gateway.websocket_bind
```

## 按问题定位

| 现象 | 先检查 |
|---|---|
| 模型无响应或请求失败 | `axon providers test <provider-name>`、`axon doctor --check provider` |
| 配置或认证异常 | `axon config validate`、`axon auth status` |
| MCP 或 Skill 不可用 | `axon mcp list`、`axon skills --summary` |
| 文件语义查询异常 | `axon lsp servers`、`axon doctor --check lsp` |
| Memory 结果异常 | `axon memory stats`、`axon memory doctor`、`axon memory search <query>` |
| 人设不生效或人格状态异常 | `axon persona validate`、`axon persona status`、`axon doctor --check persona`；人设格式无效时该轮不装配人格，回合照常继续 |
| 会话读取或恢复异常 | `axon debug session show <session-id>`、`axon debug session read <session-id>`、`axon debug session doctor` |
| Daemon、Cron 或 Remote 异常 | `axon daemon status`、`axon daemon health`、`axon cron list`、`axon remote status`、`axon remote feishu-status` |
| 停服或升级前需要优雅收口 | 先 `axon daemon drain`，用 `axon daemon health` 看 `executionBacklog.pending` 与 `deliveries.backlog.pending` 是否归零；`stop|restart` 等待窗口 10 秒，超时会强制结束进程树 |
| 事件订阅数不回落 | `axon daemon health` 的 `streamSubscribers`；再用 `scripts/gateway-ws.ps1 -Action subscribe` 对照订阅与退订是否成对 |
| 桌面工作台无窗口或行为异常 | `axon desktop` 启动入口、`axon --version`、`current.json` 指向版本目录内是否存在 `axon-desktop.exe`，以及 `axon-desktop.exe --self-check` 的单行 JSON 与退出码 |
| 需要驱动或抓取界面状态 | `--serve <端口>` 调试桥（`GET /events`、`POST /rpc`）或 CDP（`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`），细节见 [desktop-app.md](desktop-app.md) |
| HTTP 接口返回 401 | `axon config get remote.gateway.api_token_env`、实例是否已加载配置相邻 env 文件；未配置 token 时该面全部路由都是 401 |
| HTTP 接口 404 且路径含 provider | 运行根下 `events/<provider>/` 与解析脚本是否已配置 |
| HTTP 接口 411 或 415 | 补 `Content-Length`，并把 `Content-Type` 设为 `application/json` |
| HTTP 接口 429 或 503 | 连接或处理队列已满、准备失败；稍后重试并读取回执正文的错误码 |
| WS 握手 401 或 404 | token 是否与 Daemon 一致（WS 不支持查询串 token）、路径是否为 `/gateway/v1/ws` |
| WS 回 `invalid_json` | 订阅帧漏了 `workspaceId`，或字段名不是 camelCase |
| WS 回 `subscription_rejected` | 目标标识类型用错（车道标识不能当 `sessionId`），或目标已不存在 |
| WS 已 `subscribed` 但无事件帧 | 先用 `GET /gateway/v1/interactions/<id>` 确认目标状态，再区分「已结束无历史帧」与「未订阅成功」 |

## 处理原则

- 保存非敏感错误、request ID、session ID 和命令回执，便于后续核验。
- 不展示 API key、token、OAuth 凭据或外部服务密钥。
- 先从错误直接涉及的 provider、配置、Skill、MCP 或服务状态开始；只有在用户要求修复时才修改设置或重启服务。
- 写入或重启后，重新执行对应的只读检查，确认状态而不是只根据自然语言输出判断。
