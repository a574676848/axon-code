# Axon 运行能力地图

这是已安装 Axon 的任务路由表。它只列出 Agent 可观察和操作的运行能力；实际命令、参数和当前可用功能以 `axon --help` 与目标子命令的 `--help` 为准。

| 目标 | 入口 | 使用方式 |
|---|---|---|
| 交互式任务 | `axon` | 在终端中持续协作与审批；接续既有会话用 `axon run --continue` 或 `--session <id>`。 |
| 有界 Agent 任务 | `axon run` | 指定模型、cwd 或工具范围；自动化使用 `--format json` 并读取到终态事件。每次调用是一条独立主 turn 车道（独立会话表面），延续上一轮上下文必须显式 `--continue` 或 `--session <id>`。 |
| 会话诊断（运维面） | `axon debug session ...` | 先 list/show/read，再在用户授权下 resume、fork 或 delete；用户不需要也不应管理 session_id。 |
| 本地命令和文件工作 | Agent builtin tools、`axon exec` | Agent 使用 `glob`、`read_file`、`edit`、`shell_command` 等当前可见工具。`read_file` 的派生投影覆盖 Notebook、PDF、Office、压缩包清单与 SQLite（只读，对象清单 + schema + 有界数据行）；派生文档不产生快照，不能直接参与 Hashline 编辑。`read_file` 命中 PNG/JPEG/WebP/GIF 时走图片分支，把图片本身随回执交给模型，只用 `path`，并自动归一化为单帧 8-bit sRGB、按像素与字节预算降采样（源图上限 50 MiB / 6400 万像素 / 长边 65535 px）；当前模型未声明图片输入能力时该分支直接失败。 |
| 编辑契约与整文件重写 | Agent builtin tools | 改已存在的单个文件用 Hashline（`[path#TAG]` + 操作）；新建与**整文件重写**用 `*** Add File:`（目标已存在即整文件重写，回执 `operation` 为 `update` 并回传新 TAG）；删除、移动与必须原子的多文件批处理用 PathDiff。 |
| 编辑观测与恢复 | Agent builtin tools | 陈旧快照按「统一位移 + 邻域校验」重放，失败即要求重新读取；编辑把文件从可解析变为解析失败时回执 warnings 出现 `[解析回归]`（不阻断写入，需立刻修正）；编辑载荷在流式生成期有只读 diff 预览（不落盘）。 |
| 后台长任务 | Agent builtin tools | 预计长时间运行的命令用 `builtin.shell_command` 的 `run_in_background: true` 启动，回执返回 `job_id`；随后用 `builtin.job_output` 读取增量输出与终态、用 `builtin.job_kill` 终止作业。需要交互终端时改用 `tty: true` 并用 `builtin.write_stdin` 续接；`run_in_background` 与 `tty` 互斥，`job_id` 与 `session_id` 属于不同通道，不可互传。 |
| 跨窗口交接与历史恢复 | Agent builtin tools | 预算查询使用 `builtin.get_context_remaining`；主动换窗使用 `builtin.new_context`；便签使用 `builtin.notes_write_file`、`builtin.notes_append_to_file`、`builtin.notes_read_file`、`builtin.notes_list_files_by_prefix`、`builtin.notes_search_contents`；历史恢复使用 `builtin.history_list_windows`、`builtin.history_list_items`、`builtin.history_read_item`、`builtin.history_search_contents`。先成功写入便签，再请求换窗（未保存时换窗请求被拒绝并返回补写指令）；换窗后先回读便签，再按需回查历史。 |
| 配置和模型 | `axon config`、`axon providers`、`axon models`、`axon auth` | 检查或维护当前配置、连接与模型路由。 |
| Skills 与 MCP | `axon skills`、`axon mcp` | 先检查已加载项目与状态，再进行用户要求的登录或配置变更。 |
| Memory | `axon memory ...` | 搜索、检查和治理运行时记忆；破坏性操作需明确授权。 |
| 多代理协作 | Agent builtin tools | 默认**后台**委派：模型可调用的是 `delegate_to_fast`，缺省 `run_in_background: true`，立即返回 `jobId`，子代理完成时结果经完成通知回流到下一轮；相互独立的委派在同一条 assistant 消息里批量并发发起，运行期间继续做不依赖它的事（不要反复 `job_output` 空转）。仅当下一步动作依赖结果时才传 `run_in_background: false` 同步等待并直接拿到回执。`spawn_agent` 保留在执行注册表但不向模型暴露（`axon debug builtin list` 可见，模型工具清单里没有，调用它不会生效）。已存在的子代理用 `send_message` 追加消息（`intent`、`target`、`message` 三者必填，`target` 不可寻址 root；运行中→最近 step 边界读取；空闲或已恢复→重挂会话开新轮次；不可达→降级待投递，回执 `deliveryState` 与 `wakeRequired` 如实回报）、`list_agents` 巡检。生命周期走 `job_id`：`job_output` 读取结果与终态、`job_kill` 完整中断；协作寻址走 AgentPath，两者不可互传。 |
| 跨轮承诺与后台 Goal | Agent builtin tools | 建立、检索、查看、启动、推进或取消 Goal 分别用 `builtin.goal_create`、`builtin.goal_list`、`builtin.goal_get`、`builtin.goal_start`、`builtin.goal_update`、`builtin.goal_cancel`；列表按页读取，更新与取消携带 `expectedRevision`，取消还需非空原因，查询服从当前 Workspace、audience 与 disclosure policy。 |
| 用户问答交互 | Agent builtin tools | 决策确认与关键输入提问使用 `builtin.ask_user_question`，主 turn 专属；预算内等待回答，超时挂起并交还控制权。后续回答以新消息续接任务。 |
| 运行态 HTTP 接口 | `axon serve`、Gateway HTTP 面 | Gateway 面随 Daemon 常驻（`remote.gateway.http_bind`），路由族为 `/gateway/v1/*` 与 `/api/v1/*`；`axon serve` 提供 `/v1/*` 只读视图。全部需要 token 鉴权，细节见 [gateway-http.md](gateway-http.md)。 |
| 运行态 WebSocket 接口 | Gateway WS 面 | `ws://<remote.gateway.websocket_bind>/gateway/v1/ws`，握手头鉴权；`ping`、`subscribe`、`unsubscribe`、`submit` 四种客户端帧，一条连接可并存多路订阅，细节见 [gateway-websocket.md](gateway-websocket.md)。 |
| 桌面工作台 | `axon desktop`、`--self-check`、`--serve <端口>` | 图形工作台，是网关的客户端：工作区与车道、会话流订阅、Cron 与指标面板都经 `/api/v1/*` 与 WS 取得，不直连运行态数据库。自检输出单行 JSON；调试桥提供 `GET /events`、`POST /rpc`、`GET /attachment/<encoded>`。细节见 [desktop-app.md](desktop-app.md)。 |
| 用量与费率 | `axon usage`、`axon pricing`、`/api/v1/observability/*` | 看板与审计走 `axon usage`（含 `diagnostics`、`audit`、`export`）或可观测性路由；费率规则用 `axon pricing list|add|delete|sync|recalculate`。重算与写入属于状态变更。 |
| 图片生成与编辑 | Agent builtin tools、已加载 Skill | 由已加载 Skill 或其脚本调用配置中的图像模型完成，可能产生费用，先确认用户意图；公网搜索统一由 MCP/Skill 连接器（如 anysearch）扩展。 |
| Daemon、Cron、Remote | `axon daemon`、`axon cron`、`axon remote` | `axon daemon status` 只回答注册与运行；健康快照用 `axon daemon health`（`host.status`、`executionBacklog.pending`、`deliveries.backlog.pending`、`streamSubscribers`）。优雅收口先 `daemon drain` 再确认积压归零，最后 `daemon stop`；`stop|restart` 先写停止请求让 Daemon 自行收口（15 秒），未响应才强制结束进程树。启动、停止、投递和执行任务属于状态变更。 |
| 诊断与升级 | `axon doctor`、`axon lsp`、`axon upgrade` | 从只读诊断开始，升级前检查候选与影响。 |

不要把 TUI 里的斜杠命令当作 CLI 子命令。MCP 工具 schema 随每轮请求全量下发，外部能力直接在工具清单中按完整 MCP 工具名调用；清单中没有对应工具即判定该能力不可用，不得臆造。

- 人格：`axon persona validate|status|style` 校验人设并查看人格状态，`axon doctor --check persona` 给出同一判定；人设格式无效时该轮不装配人格，回合照常继续，告警随终态 warnings 返回。
