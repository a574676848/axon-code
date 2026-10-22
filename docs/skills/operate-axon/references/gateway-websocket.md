# Axon WebSocket 接口

本文件只描述已安装 Axon 在运行态暴露的 WebSocket 面：监听地址、握手鉴权、帧协议与排障口径。所有结论以当前实例的实际回帧为准。

## 面概览

```text
ws://<remote.gateway.websocket_bind>/gateway/v1/ws
```

- 监听地址由 `axon config get remote.gateway.websocket_bind` 决定，与 HTTP 面是**不同端口**。
- 由 Daemon 提供，随 Daemon 启动；先用 `axon daemon status` 确认 `running` 再连接。
- 一条连接可并存多路订阅，各自用 `subscribeId` 标识；断开连接即整体释放全部订阅。

## 握手与鉴权

| 情况 | 结果 |
|---|---|
| 路径不是 `/gateway/v1/ws` | 握手返回 `404`，体为 `{"error":"not_found"}` |
| 缺少或不匹配 `Authorization: Bearer <token>` | 握手返回 `401`，体为 `{"error":"unauthorized"}` |
| 鉴权通过 | `101` 升级成功，服务端不发送任何欢迎帧，直接等待客户端帧 |

- WS 面**只接受握手头**携带 token；HTTP 面的 `?api_token=` 查询串通道在这里不适用。
- token 名与取值来源与 HTTP 面一致：`remote.gateway.api_token_env` 指定的环境变量，其次配置相邻 env 文件。
- 并发连接数达到 Gateway 上限（默认 256）时，握手仍会升级成功，随后立即回 `{"type":"error","error":"gateway_busy"}` 并关闭连接。

## 报文上限

- 文本帧上限 16 MiB、整条消息上限 64 MiB；超过时回 `{"type":"error","error":"message_too_large"}`，连接保持。
- 二进制帧一律拒绝，回 `{"type":"error","error":"text_message_required"}`。
- 协议级 `Ping` 帧由服务端回 `Pong`，不计入业务帧。
- 所有客户端帧都不接受未知字段；字段拼写错误会被拒为 `invalid_json`，不会静默忽略。

## 客户端帧

```json
{"type":"ping"}
{"type":"subscribe","subscribeId":"interaction:<interaction-id>","workspaceId":"<workspace-id>","interactionId":"<interaction-id>","resumeCursor":"<cursor>"}
{"type":"subscribe","subscribeId":"session:<session-id>","workspaceId":"<workspace-id>","sessionId":"<session-id>"}
{"type":"unsubscribe","subscribeId":"<subscribe-id>"}
{"type":"submit","sourceChannel":"jira","eventId":"AX-1024","workspaceId":"<workspace-id>","observedAtMs":1730000000000,"expiresAtMs":1730086400000,"content":"事件正文","replyChannel":"jira","replyDestination":"AX-1024","localFiles":[]}
```

| 帧 | 必填字段 | 可选字段 | 回执 |
|---|---|---|---|
| `ping` | 无 | 无 | `{"type":"pong"}` |
| `subscribe` | `subscribeId`、`workspaceId`，以及 `interactionId` 或 `sessionId` 之一 | `resumeCursor` | `{"type":"subscribed","subscribeId":"..."}` |
| `unsubscribe` | `subscribeId` | 无 | `{"type":"unsubscribed","subscribeId":"..."}` |
| `submit` | `sourceChannel`、`eventId`、`observedAtMs`、`expiresAtMs`、`content`、`replyChannel`、`replyDestination` | `workspaceId`、`localFiles` | `{"type":"accepted","receipt":{"interactionId":"...","accepted":true,"deduplicated":false}}` |

订阅目标语义：

- 给出 `sessionId` 时按**会话级**订阅处理，此时 `interactionId` 被忽略。
- `sessionId` 是子代理会话的会话标识（形如 `parent--subagent--root`），**不是**车道标识；把车道标识当 `sessionId` 传会被拒为 `subscription_rejected`。主会话车道用 `interactionId` 订阅，交互标识可从 `GET /api/v1/lanes/<laneId>/interactions` 取得。
- `subscribeId` 由客户端生成，必须在同一连接内唯一；建议直接由目标推导（`interaction:<id>` 或 `session:<id>`）。
- `resumeCursor` 用于从既有游标续读；省略时从头回放。

## 服务端帧

```json
{"type":"pong"}
{"type":"subscribed","subscribeId":"interaction:<interaction-id>"}
{"type":"unsubscribed","subscribeId":"interaction:<interaction-id>"}
{"type":"accepted","receipt":{"interactionId":"...","accepted":true,"deduplicated":false}}
{"type":"event","subscribeId":"interaction:<interaction-id>","event_type":"wire_event","payload":{...},"cursor":"<cursor>","replay":true}
{"type":"error","error":"<code>"}
{"type":"error","error":"<code>","subscribeId":"<subscribe-id>"}
```

事件帧的字段命名是混合的，解析时不要按单一风格推断：`type` 与 `subscribeId` 为 camelCase，`event_type`、`payload`、`cursor`、`replay` 为 snake_case；`payload` 是**内嵌的 JSON 对象**，不再套一层字符串转义。`replay` 为 `true` 表示历史回放帧。

订阅一个**已经终态**的 Interaction 时，这次回放以一条终态帧收口，且排在全部 durable 事件之后：

```json
{"type":"event","subscribeId":"interaction:<interaction-id>","event_type":"interaction.status","payload":"interrupted","cursor":null,"replay":true}
```

`payload` 是状态字符串（`"understood"`、`"rejected"`、`"interrupted"`），`cursor` 为空。终态平时只由实时通道在状态跃迁时发出；早已结束、甚至从未产生过事件的回合不会再跃迁，客户端只有靠这条补帧才能收口当前轮次。`received` 与 `processing` 不补这条帧：那一轮还在跑，由实时状态跃迁给出。

## 错误码

| `error` | 触发条件与处置 |
|---|---|
| `invalid_json` | 帧不是合法 JSON、字段拼写错误或缺少必填字段（含 `subscribe` 缺 `workspaceId`）；按上面的帧表逐字段核对 |
| `message_too_large` | 文本帧超过 16 MiB（或整条消息超过 64 MiB）；拆分或改用 HTTP 面提交 |
| `text_message_required` | 发送了二进制帧；改发文本帧 |
| `gateway_busy` | 连接数达上限；稍后重试 |
| `event_rejected` | 提交的事件被入口策略拒绝，未入账 |
| `subscribe_missing_id` | `subscribe` 未带 `subscribeId`；该帧不回显标识 |
| `subscribe_duplicate_id` | 同一连接复用了 `subscribeId`；只拒这一次，连接保持 |
| `subscription_rejected` | 目标不存在或不可订阅；回显 `subscribeId`，Daemon 侧日志留有具体原因 |
| `event_projection_failed` | 事件投影失败；服务端以错误帧显式回执，不静默丢帧 |

## 排障对照

| 现象 | 先检查 |
|---|---|
| 连接直接失败并报 401 | token 是否与 Daemon 一致；WS 不走查询串 token |
| 连接直接失败并报 404 | 路径必须精确为 `/gateway/v1/ws` |
| 连上后无任何回帧 | 这是正常行为：服务端不发欢迎帧，先发 `ping` 或 `subscribe` |
| `subscribe` 回 `invalid_json` | 漏了 `workspaceId`，或字段名不是 camelCase |
| `subscribe` 回 `subscription_rejected` | 目标标识类型用错（车道标识当 `sessionId`）或目标不存在 |
| 有 `subscribed` 但长时间没有事件帧 | 目标 Interaction 已结束且无历史帧；先用 `GET /gateway/v1/interactions/<id>` 确认状态 |
| 事件帧解析失败 | `payload_json` 是字符串化 JSON，且同帧内命名风格混合 |
| 界面报「连接被重置」（`WebSocket protocol error: Connection reset without closing handshake`） | `data/logs/axon-<日期>.log` 的 `[gateway-ws] 连接级失败 peer=... 原因=...`：有该行是服务端读到了非正常断开，无该行说明断在链路层 |

## 调用脚本

与 `SKILL.md` 同级的 `scripts/gateway-ws.ps1` 完成握手、发送一帧、按窗口收帧并打印，结束时自动退订并正常关闭。

```powershell
# 连通性与鉴权
pwsh -File scripts/gateway-ws.ps1 -Action ping

# 订阅主会话车道（interaction 级）
pwsh -File scripts/gateway-ws.ps1 -Action subscribe -WorkspaceId <workspace-id> `
  -InteractionId <interaction-id> -SubscribeId "interaction:<interaction-id>" -ListenMs 5000

# 订阅子代理会话（session 级）
pwsh -File scripts/gateway-ws.ps1 -Action subscribe -WorkspaceId <workspace-id> `
  -SessionId <session-id> -SubscribeId "session:<session-id>"

# 经 WS 提交一条外部事件
pwsh -File scripts/gateway-ws.ps1 -Action submit -SourceChannel jira -EventId AX-1024 `
  -Content "事件正文" -ReplyChannel jira -ReplyDestination AX-1024
```

判定口径：输出以 `OK:ws` 或 `ERR:ws` 开头，收帧逐行打印为 `FRAME <原始帧>`；退出码 0 表示握手成功且订阅时收到了 `subscribed`，1 表示其他情况。

## 相关面：桌面调试桥

桌面端在显式传入 `--serve` 时才监听 `127.0.0.1:<port>`（不传参不监听任何端口），只绑 loopback、无鉴权，用于在浏览器中验证与驱动界面：

| 方法与路径 | 用途 |
|---|---|
| `GET /events` | 与桌面窗口同一批会话帧的 SSE 流 |
| `POST /rpc` | 体为 `{"cmd":"<命令>","args":{...}}`，回执为 `{"ok":true,"data":...}` 或 `{"ok":false,"error":"..."}` |
| `GET /attachment/<encoded>` | 读取附件 |

它与 Gateway 的两个面消费同一批会话帧，因此可用来对照排查「界面没刷新」还是「订阅没收到帧」。
