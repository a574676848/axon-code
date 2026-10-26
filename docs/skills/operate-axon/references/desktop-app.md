# Axon 桌面工作台（axon-desktop）

本文件描述已安装 Axon 的桌面工作台在运行态可观察与可操作的入口、它消费的运行态面、状态文件、自检与调试桥。取值以当前实例的实际回执为准。

## 启动与入口

| 入口 | 说明 |
|---|---|
| `axon desktop` | 稳定入口：经安装根 launcher 转发到 `current.json` 指向版本目录内的桌面二进制；launcher 会脱离控制台并把标准输入输出丢弃 |
| `axon-desktop.exe --self-check` | 单行 JSON 自检，退出码 0 成功、1 失败 |
| `axon-desktop.exe --serve <端口> [静态目录]` | 启动本地调试桥，端口省略时取 `14222` |
| 不带参数运行版本目录内二进制 | 启动工作台窗口 |

- 工作台是**网关的客户端**：数据一律经网关协议取得，不直连运行态数据库。
- 直接调用版本目录二进制只用于自检与调试桥；日常启动一律走 `axon desktop`，以便升级换目录后快捷方式仍有效。

## 它消费的运行态面

| 面 | 用途 |
|---|---|
| `GET /api/v1/workspaces`、`GET /api/v1/lanes` | 工作区与车道列表 |
| `POST /api/v1/workspaces`、`POST /api/v1/lanes/identity` | 注册工作区、推导车道标识 |
| `POST /api/v1/lanes/<laneId>/interrupt` | 中断该车道当前运行 |
| `POST /api/v1/lanes/<laneId>/jobs/<jobId>/stop` | 停止该车道的后台作业 |
| `POST /api/v1/lanes/<laneId>/subagents/<agent>/messages` | 给该车道的子代理追加消息 |
| `GET /api/v1/lanes/<laneId>/interactions`、`/jobs`、`/subagents` | 车道详情 |
| `ws://<remote.gateway.websocket_bind>/gateway/v1/ws` | 会话事件流，按 `subscribeId` 并存多路订阅 |
| `GET /api/v1/crons` 系列 | Cron 管理与运行历史 |
| `GET /api/v1/observability/*` | 指标与审计看板（设置 › 用量统计，按概览、实时、活动、诊断、请求日志分页签） |
| `GET/POST /api/v1/observability/pricing/*` | 用量看板的成本口径与费率规则（设置 › 模型定价，含每日自动同步开关） |
| `POST /api/v1/mcp/status`、`/refresh` | 指标条的 MCP 读数与输入坞的 `/mcp`：读 daemon 启动快照里的加载结论，`/refresh` 才会重探 |

工作台打开的车道订阅会计入 `axon daemon health` 的 `streamSubscribers`：切换车道与关闭窗口后该数应回落到 0，不回落说明订阅未释放。

## 正文图文渲染

助手正文里的图片按内容直接渲染，判据只有图片后缀与体积，不限定图片所在目录。

| 写法 | 呈现 |
|---|---|
| `![说明](路径)` | 内联图片；路径含空格时用 `![说明](<路径>)` |
| `![说明](https://...)` 或 `http://...` | 由 webview 直接加载，不经本地读取通道 |
| 裸文本里的图片路径 | 图片路径芯片；悬浮出缩略图，点击在右侧栏打开 |

- 相对路径以当前工作区根为准，与输入坞 `@` 引用同作用域；`~/` 与盘符开头的路径原样交给后端。
- 读取通道与附件卡同一条：桌面窗口走 IPC 数据地址（`attachment_preview`），调试桥走 `/attachment/`。图片后缀清单为 png、jpg、jpeg、webp、gif、bmp、ico、svg、tif、tiff、avif、heic，单张上限 8 MiB。
- 读不出来时给一张破图占位并附上原路径，不把说明文字混进正文；图片进入视野后才读字节，同一路径的读取结果在最近这些张之内复用，更早的被移出后再次显示会重新读一次。
- 预览按图片路径本身放行，不限定所在目录：判据是已知图片后缀与 8 MiB 上限，非图片与不存在的路径一律读不出来。

## 历史加载与折叠

- 车道历史按「一次一条 Interaction」加载：打开会话只取最近一条，更早的由正文顶部的「加载更早」按需取；取到空页即到底，按钮消失。前置一页时正文区按实际高度差补偿滚动位置，阅读位置不跳。
- 历史轮次按两条判据回收，与 TUI 的 cell history GC 同一口径：**体量**上单轮正文过大（工具输出、推理、diff）即折，**窗口**上只保留最近 8 轮展开，更早的一律折成一行摘要。两条判据都只作用于老轮次：最近两轮（正在跑的一轮与上一轮）始终展开，因此发送新一轮不会把上一轮折起来。摘要形如「用户那一句」加动作短语（例如「已读取文件，已搜索代码，修改了文件等」）。
- 折叠是**释放**而不是隐藏：折叠后该轮的工具输出与推理正文不再留在界面内存里，展开时按 Interaction 标识重新回放后才换回正文。正在运行的一轮、以及用户点开过的一轮不会被自动回收。
- 界面长时间没有心跳（窗口可见且未最小化时静默超过 20 秒）会被宿主切到随包发布的错误引导页 `error.html`：页面给出失败类型、可重新加载界面、可复制诊断信息。会话、任务与运行状态都在 Daemon 里，不受界面重新加载影响。

## 运行态文件

相对 Axon 运行根（`axon config path` 所在目录的父级，通常为 `~/.axon`）：

| 路径 | 内容 |
|---|---|
| `desktop-state.json` | 桌面状态：最近工作区、通知开关等 |
| `data/runtime/draft_history.jsonl` | 输入历史，与 TUI 同名同格式 |
| `data/attachments/` | 附件暂存目录 |
| `gateway-status.json` | 网关探测状态；缺失时按未探测处理 |

## 调试桥

只在显式传 `--serve` 时监听 `127.0.0.1:<端口>`（缺省 `14222`），仅绑定 loopback、不校验鉴权；正常启动不监听任何端口。

| 方法与路径 | 用途 |
|---|---|
| `GET /events` | 与桌面窗口同一批会话帧的 SSE 流 |
| `POST /rpc` | 体为 `{"cmd":"<命令>","args":{...}}`，回执 `{"ok":true,"data":...}` 或 `{"ok":false,"error":"..."}`；未知命令回 `未知命令：<cmd>` |
| `GET /attachment/<encoded>` | 读取图片字节；`<encoded>` 为百分号编码的图片路径，按图片后缀与 8 MiB 上限判定，不符返回 404 |
| 其余 `GET` | 传入静态目录时按目录返回静态文件 |

当前可调用的命令族：

| 族 | 命令 |
|---|---|
| 配置 | `config_read`、`config_validate`、`config_validate_values`、`config_write`、`config_apply_values`、`config_reset_key`、`open_config_file` |
| Provider 与模型 | `provider_profile_upsert`、`provider_profile_remove`、`discover_models` |
| 工作区与车道 | `workspace_list`、`workspace_add`、`workspace_remove`、`workspace_list_entries`、`workspace_read_file`、`workspace_read_bytes`、`workspace_stat_path`、`workspace_applications`、`workspace_open_default`、`workspace_open_with`、`workspace_reveal`、`lane_create`、`lane_rename`、`lane_reorder`、`lane_set_pinned`、`lane_set_archived` |
| 会话 | `conversation_submit`、`conversation_history`、`conversation_watch`、`conversation_watch_session`、`conversation_unsubscribe`、`conversation_interrupt`、`conversation_stop_job`、`conversation_send_subagent_message`、`conversation_lane_jobs`、`conversation_lane_subagents` |
| Cron | `cron_list`、`cron_config`、`cron_get`、`cron_history`、`cron_create`、`cron_update`、`cron_pause`、`cron_resume`、`cron_run`、`cron_remove` |
| MCP 与 Skill | `mcp_list`、`mcp_upsert`、`mcp_upsert_raw`、`mcp_remove`、`mcp_runtime_status`、`mcp_runtime_refresh`、`skills_list` |
| 文档与输入历史 | `document_catalog`、`document_read`、`document_write`、`open_document`、`open_external_url`、`input_history_list`、`input_history_append` |
| 凭据 | `secret_status_of`、`secret_set`、`secret_clear` |
| 系统与偏好 | `system_info`、`check_update`、`gateway_health`、`gateway_start_service`、`desktop_preferences`、`desktop_set_notifications`、`pick_directory`、`attachment_stage` |

- 读类命令可直接用于排障（`system_info`、`gateway_health`、`conversation_history`、`cron_history`）。
- 写类命令（`config_write`、`config_apply_values`、`secret_set`、`mcp_upsert`、`cron_*`、`lane_*`、`conversation_submit`、`conversation_interrupt` 等）都是状态变更，先取得用户明确授权。
- 凭据类命令只回报状态与掩码，不打印密钥原文。
- `system_info` 的 `github` 字段给出本次构建对应的仓库、发布页、问题反馈与许可标识（都来自构建期元数据）；运行状态在同一条回执的 `services`、`daemon_running`、`remote_feishu` 里。
- `check_update` 按该仓库读取最新发布。失败不是命令失败：`error` 是中文归因、`hint` 是下一步动作，两者都不含 HTTP 状态码或库内部字符串；界面上归因收在「关于」页的「详情」里。
- 读取私有仓库需要令牌：配置目录 `.env` 中的 `AXON_GITHUB_TOKEN`（或 `GH_TOKEN`、`GITHUB_TOKEN`）。未配置时按匿名请求读取，额度为每小时 60 次，超限的 `hint` 会给出重置倒计时。
- `open_external_url` 只接受 `http` 与 `https`，其余协议直接拒绝。

## 指标条读数与输入坞指令

| 位置 | 内容 |
|---|---|
| 指标条辅助簇 | `MCP <已加载台数>`；有服务器没加载起来时写 `MCP <正常> · <异常> 异常` 并整段转危险色，悬停给出总台数与原因；一台都没有且没有错误时不占位 |
| 输入坞 `/mcp` | 清 MCP 健康负缓存并强制重建启动快照，也就是重连一遍 MCP；执行期间输入坞上方显示 `正在重载 MCP 服务器…` 与已耗时，返回后换成一行回执，同时在输入坞上沿摊开服务器清单 |

- `/mcp` 的回执只有四种口径：`已重载 MCP：N 台全部正常`、`已重载 MCP：N 台正常，M 台异常`、`已重载 MCP：这个工作区没有配置 MCP 服务器`、`重载 MCP 失败：<原因>`。
- 执行态复用工作状态行（进行态转圈 + 文案 + 已耗时 + 底部扫光），但不给 `按 ESC 中断` 提示：本机指令不可打断；执行期间重复提交同一条指令会被忽略。
- 清单逐台列出：加载成功的写服务器名、工具数与工具名（等宽、最多两行）；失败的写服务器名与失败原因，并排在清单最前。清单贴输入坞上沿、与输入坞同宽，超出高度在清单内部滚动，关闭按钮或下一次提交即收起。
- 读数与 `/mcp` 都按**当前工作区**取数（各工程的 `.mcp.json` 各算各的），并且不轮询：进入工作区读一次，只有敲 `/mcp` 才会重探。
- 读不到结论（网关不可达、daemon 未运行）时指标条显式写 `MCP 异常` 并给原因，不隐藏这一段。
- `/mcp` 只在正文恰好是这一条指令时生效；`/mcp 带参数` 按普通消息发给模型。

## 界面级调试（CDP）

启动窗口前设置调试端口，再启动桌面端：

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9333'
axon desktop
```

- 端口可用 `GET http://127.0.0.1:9333/json` 列出页面目标；`Runtime.evaluate` 读回 DOM 与状态，`Input.dispatchMouseEvent` 派发真实鼠标事件，`Input.insertText` 输入文本。
- 收尾必须关闭该环境变量（新开 shell 或清空），不得把调试端口带进日常使用。

## 排障对照

| 现象 | 先检查 |
|---|---|
| `axon desktop` 没有窗口 | `axon --version` 是否可用；`current.json` 指向版本目录内是否存在 `axon-desktop.exe` |
| 界面显示未连接 | `axon daemon status` 与 `axon daemon health`；网关监听地址与 token（见 [gateway-http.md](gateway-http.md)） |
| 无法判断桌面端自身状态 | `axon-desktop.exe --self-check` 的单行 JSON 与退出码 |
| 调试桥连不上 | 是否显式传了 `--serve`；端口是否被占用；基址必须是 `127.0.0.1` |
| 界面内容与预期不符 | 桌面二进制内嵌前端产物：确认构建顺序为**先重建前端产物、再重建桌面端** |
| 订阅数不回落 | `axon daemon health` 的 `streamSubscribers`；配合 [gateway-websocket.md](gateway-websocket.md) 的订阅脚本核对成对释放 |
