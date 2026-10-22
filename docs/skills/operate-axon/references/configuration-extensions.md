# Axon 配置与扩展

## 配置和模型

先定位并验证正在使用的配置：

```text
axon config path
axon config env-path
axon config validate
axon providers --help
axon models --help
```

- 用户指定 `--config`、provider 或 model 时，原样使用并在执行前检查帮助。
- 凭据使用环境变量或配置相邻的环境文件保存；只读取认证状态，不显示实际密钥。
- 改写配置前确认目标文件、变更内容和预期影响；写入后再次运行 `config validate`。

## 运行期开关（环境变量）

- `AXON_FILTER_STRATEGY`：`auto` / `minimal` / `aggressive`，控制 `read_file` 的结构化省略强度；启用省略时对应页面不产生 Hashline 快照（折叠区不可编辑，需按完整页重读）。
- **生效范围（实测）**：`axon run` 的回合由守护进程执行，因此运行期环境变量只设在当前 shell 里对 `axon run` **不生效**；需要写入 `~/.axon/.env`（守护进程的启动环境）并 `axon daemon restart`，才会随新一轮的工具 schema 与上下文下发。

## Skill 与 MCP

```text
axon skills --summary
axon mcp list
axon mcp get <server-id>
```

- `axon skills --summary` 中的有效 Skill 才是可调用集合。Skill 是操作说明，不是远程动作接口。
- 需要外部能力时，Agent 直接在已下发的工具清单中确认可调用的 MCP 工具并调用该具体工具。
- 添加或移除 MCP，或进行 OAuth 登录和退出，都会改变运行环境，必须在用户要求范围内操作并随后检查状态。

### MCP OAuth（Streamable HTTP）

```text
axon mcp login <server-id>            # 生成授权 URL，不启动本地回调
axon mcp login <server-id> --browser  # 拉起浏览器并等待本地回调，成功后保存 token
axon mcp logout <server-id>           # 清除本地 token 与待完成会话
```

- 授权元数据按 MCP `2026-07-28` 顺序解析：401/403 challenge → endpoint 子路径 PRM → 站点根 PRM。
- 需要授权时 CLI 与 TUI `/mcp` 会直接给出登录引导；TUI 内用 `/mcp login <server-id>`，不会自动弹浏览器。
- 动态客户端注册结果按「授权服务器 + redirect URI」缓存在 `~/.axon/mcp-oauth/clients/`，同一授权服务器只注册一次。
- 凭据保存在 `~/.axon/mcp-oauth/`，只读取状态，不显示 token 值。

## 配置键全表

`axon config keys` 列出全部可编辑键及其类型；`axon config get <键>` 读取当前生效值，`axon config set <键> <值>` 写入，改写后运行 `axon config validate`。

### Agent 与子代理

| 键 | 作用 |
|---|---|
| `agent.default_model` | 主 Agent 默认模型，引用已配置 model 的 name 或 id |
| `agent.fast_model` | 快速委托模型，被子代理与快速委派使用 |
| `agent.reasoning_effort` | 主 Agent 默认模型的推理强度，仅作用于 `default_model` |
| `agent.log_level` | 日志级别：`trace`、`debug`、`info`、`warn`、`error` |
| `agent.max_subagent_threads` | 每个 run session 允许的最大子代理线程数 |
| `agent.max_subagent_active_turns` | 每个 run session 允许同时运行的子代理 turn 数 |
| `agent.max_subagent_depth` | 子代理最大嵌套深度，root 深度为 0 |
| `agent.idle_compaction_debounce_ms` | 闲暇压缩 debounce 窗口，单位毫秒 |

并发委派超过 `max_subagent_threads` 或活跃 turn 超过 `max_subagent_active_turns` 时，超额请求会被准入裁决拒绝并回流明确原因，而不是排队等待。

### 渠道、记忆与远控

| 键 | 作用 |
|---|---|
| `channel_balancer.failure_threshold` | 连续失败达到该次数后熔断当前通道 |
| `channel_balancer.cooldown_ms` | 熔断后的冷却时间 |
| `channel_balancer.affinity_ttl_ms` | 同一任务复用通道亲和关系的时间 |
| `channel_balancer.telemetry_persistence_enabled` | 是否把渠道遥测样本持久化到 `data/channel-metrics` |
| `memory.embedding.enabled` | 是否启用长期记忆 embedding 检索 |
| `memory.embedding.model` | Memory 嵌入向量模型，引用顶层 model catalog 的 name 或 id |
| `remote.mode` | 远控运行模式，`standard` 适合默认场景，`high-throughput` 适合高频事件流 |
| `remote.gateway.api_token_env` | Gateway API token 的环境变量名 |
| `remote.gateway.http_bind` | Gateway HTTP 面监听地址 |
| `remote.gateway.websocket_bind` | Gateway WebSocket 面监听地址 |

### Feishu WebSocket 远控

| 键 | 作用 |
|---|---|
| `remote.feishu_websocket.enabled` | 是否启用 Feishu WebSocket 远控 adapter |
| `remote.feishu_websocket.app_id` | 飞书应用 app_id |
| `remote.feishu_websocket.app_secret_env` | app_secret 的环境变量名 |
| `remote.feishu_websocket.api_base_url` | OpenAPI 基址，留空用官方默认地址 |
| `remote.feishu_websocket.websocket_url` | 长连接地址，留空用官方默认地址 |
| `remote.feishu_websocket.enable_notification_sending` | 是否允许该 adapter 发送远控通知 |
| `remote.feishu_websocket.notification_chat_id` | 后台完成通知的 chat_id，留空不发送 |
| `remote.feishu_websocket.allowed_sender_open_ids` | 允许访问远控的 sender open_id 白名单 |
| `remote.feishu_websocket.blocked_sender_open_ids` | 禁止访问的 sender open_id 黑名单，优先级高于白名单 |
| `remote.feishu_websocket.require_mention` | 群聊是否要求显式 @ 机器人后才进入远控 |

启用或调整 Feishu 远控属于状态变更：先确认目标与影响范围，改完用 `axon remote status`、`axon remote feishu-status` 核验，再按需 `axon daemon restart` 让 Daemon 重新加载配置。

## 图片与外部扩展

- 图片生成、图片编辑或外部投递可能产生费用或副作用，应先取得用户明确意图；联网搜索等外部扩展能力统一通过 MCP 或 Skill 连接器（如 anysearch）处理。
- 修改 Daemon、Cron 或 Remote 配置前，先运行对应的 `--help` 与 `status`/`list`，完成后以回执或 status 验证。
