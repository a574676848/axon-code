# v0.0.1 真机验收记录

> 验证对象：安装态版本树 `$HOME/.axon/bin/versions/0.0.1/`，与 `target/release/` 逐字节核对一致
> （本轮 `axon.exe` 与 `axon-desktop.exe` 均 hash-match，安装走 `scripts/install-from-source.ps1 -Components all`）。
> 绑定关系：安装态二进制构建自提交 `000c25f30`；`axon.exe` SHA-256 `A94602B615BAC5C4DD751203A253D3AD43D1F04E5A5F8FFA7D127545EADFD5A8`，
> `axon-desktop.exe` SHA-256 `3A10D7C3074771B69FE80EFA9A381DE97027E98D2A9CBFABA617DF01A9A64FB5`。
> 隔离方式：`AXON_RUNTIME_ROOT` 指向 `$env:TEMP/axon-verify-rt`。目录名不能以 `.axon` 结尾，否则 CLI 会判成
> 默认运行态根、端点退化成连不上的 `axon-daemon`。根内含 `config.toml`、`.env`、`skills/` 与 `.mcp.json` 的副本；
> 缺少 `.mcp.json` 时桌面状态栏会显示 MCP 0，补齐并重启 daemon 后恢复 MCP 3。
> 真实 `$HOME/.axon/config.toml` 前后 SHA-256 均为 `2208FC66…6646`，未被改动。
> 判据：只看输出文本与可回读的状态，不看退出码；未取得证据的条目一律标注「未真机验证」。

## 基础（B）

| # | 检查 | 结果 | 证据 |
| --- | --- | --- | --- |
| B1 | `axon --version` | 通过 | `axon-cli 0.0.1` |
| B2 | `Get-Command axon` | 通过 | 解析到 `C:\Users\<用户>\.axon\bin\axon.cmd`，不是 cargo bin |
| B3 | `axon doctor` | 通过 | `Axon doctor: ok`；config/cli/preflight/provider/tool/mcp/memory/persona/lsp/smoke 全项 ok（`channels=11 models=16 registered_tools=37`） |
| B4 | `data/` 布局 | 通过 | 空临时根起跑后 `data/` 下只有 `control.sqlite3`、`outbox.sqlite3` 与运行态子目录；递归查 `rollout.sqlite3` 零命中 |
| B5 | 多回合写入 | 通过 | 每段会话生成 `data/events/<会话>/session.v1.jsonl.zstd` 与 `.lock`；Thread 事件落 `data/sessions/threads/<hex>`，索引为 `data/sessions/thread-index.v1.*` |
| B6 | Daemon 重启 3 次 | 通过 | 连续三次停/起，`daemon health` 每次回 `"status":"running"`；当日日志中 `database is locked` 与 `Fenced` 命中数为 0；桌面在三次重启后仍能读回正文 |
| B7 | 24 小时长跑 | 观测进行中 | 已启动 24 小时观测（`tools/verify-soak.ps1`，5 分钟采样，证据落在 `%TEMP%\axon-soak\samples.jsonl`）。首轮采样：daemon 存活、工作集 32 MB、`database is locked` 与 `Fenced` 的**本次窗口增量**均为 0（当日更早的 7 次 locked 发生在安装前，按基线抵消，不作本次结论） |

## 桌面（D1 至 D12）

| # | 检查 | 结果 | 证据 |
| --- | --- | --- | --- |
| D1 | 启动 | 通过 | 主窗口出现，左侧会话菜单渲染工作区与两条会话，header 指标区显示「本机 · MCP 3 · 技能 28 · N 毫秒」 |
| D2 | 新建会话 | 通过 | 点「新会话」后侧栏新增条目并选中；发出首条消息后控制面出现该会话行（`turns=1`）且 `data/events/<会话>/` 目录创建 |
| D3 | 正常对话 | 通过 | 流式输出可见（思考、记忆检索等过程行），header 出现耗时、累计、上下文、命中率、首字与速率读数 |
| D4 | 工具调用 | 通过 | 工具行渲染出命令、`exit_code=0` 与 `stdout=d4-ok`；读取行显示路径与耗时 |
| D5 | 子代理 | 通过 | header 出现「子代理 1」；点开名册行进入子代理详情（返回条 + 任务标题 + 模型），状态呈终态 |
| D6 | 后台任务 | 通过 | 作业 `job_id=1` 从 running 到 completed，收到「任务已完成」通知并给出终态、退出原因与完整结果路径 |
| D7 | 切换会话 | 通过 | 点另一会话后正文整体换成该会话历史，无上一会话残留 |
| D8 | 加载历史会话 | 通过 | 冷启动（重载窗口）后打开旧会话，5 轮正文按序加载，指标区与末轮读数一致 |
| D9 | 导航条 | 通过 | 置顶后该会话排到首位且 `sessions.pinned=1`；取消置顶后顺序恢复 |
| D10 | header 指标 | 通过 | `session_metrics` 行（`turn_count=5`、`total_active_ms=19926`、`channel_mix_json`）与 header 的轮次、耗时、token 读数同源；TTFT 与速率取本轮实测 |
| D11 | 归档会话 | 通过 | 归档后该会话从默认列表消失；`axon sessions list --all` 仍能找回并标注 `[已归档]` |
| D12 | 来源渠道可见 | 通过 | 每轮用户气泡页脚标注「来源 桌面 / 来源 CLI / 来源 定时」，取该轮的**发起客户端**；同一条桌面车道被 CLI 续聊时，续聊那一轮标「来源 CLI」（实测见 X6/X7） |

## CLI 会话（C1 至 C6）

| # | 检查 | 结果 | 证据 |
| --- | --- | --- | --- |
| C1 | `axon sessions list` | 通过 | 每段对话一行，含标题、最近活动与轮次；不再出现「有轮次 + 空壳」两份 |
| C2 | `run --session <清单 ID>` | 通过 | 直接取 `sessions list` 的会话 ID 追加：回复 `收到`，该会话 `turns` 4→5，不新增会话行 |
| C3 | `--session` 指向不存在的 id | 通过 | `error: 会话不存在：session-does-not-exist。先用 axon sessions list 查看可用会话；--session 不会隐式新建会话` |
| C4 | 同 cwd 连续两轮 | 通过 | 第一轮回 `收到`，第二轮问「我刚让你记住的数字」回 `41`，落在同一会话 |
| C5 | `resume` 后 `run --continue` | 通过 | `sessions resume <清单 ID>` 输出 `已恢复 Thread: …` 与 `- session: …`；随后 `--continue` 回 `41`，`turns` 2→3 |
| C6 | `--session` 与指针冲突 | 通过 | 指针指向 B 时 `--session A` 生效（A 4→5）；随后 `--continue` 仍指向 B 并被跨工作区护栏拦下；回到 B 工作目录 `--continue` 回 `77`（B 1→2） |

## 跨端（X1 至 X7）

本环境无交互式终端，TUI 侧无法取值；跨端场景用 **桌面 + CLI** 覆盖等价链路，TUI 参与的部分如实标注。

| # | 检查 | 结果 | 证据 |
| --- | --- | --- | --- |
| X1 | 一端在聊，另一端载入同一会话 | 通过（桌面 ↔ CLI） | 桌面新增一轮后，`axon sessions list` 与 `debug session show` 读到同一 Thread 的完整轮次；`resume` 后 CLI 能接续同一会话 |
| X2 | CLI 在聊，桌面载入同一会话 | 通过（CLI ↔ 桌面） | CLI 追加一轮（`turns` +1）后，桌面点开该会话能看到该轮正文与回复 |
| X3 | 两端同时在线，一端发消息 | 通过（桌面 + CLI） | 桌面停在该会话不动，CLI 用 `axon run --session <会话 ID>` 追加一轮；随后同屏多出这一轮的提问与回复，页脚标「来源 CLI」，全程没有刷新窗口。此前该会话两轮，之后三轮 |
| X4 | 一端中断会话 | 通过（桌面中断，CLI 观察） | 桌面提交「用 shell 跑 Start-Sleep 40」后点「停止生成」，正文出现「本轮已由用户中断。」（用时 21.28s）；Thread 事件里该轮以 `turnFailed` 收口且 `failure.category=interrupted`、`code=run_interrupted`、`redactedMessage=本轮已由用户中断。`；CLI `axon sessions read` 读到该轮终态、空回复与用时。该命令原先把中断显示成 `Failed`，已改为 `Interrupted`（随本版发布） |
| X5 | 一端新建会话 | 通过（桌面 ↔ CLI） | 桌面新建后 `axon sessions list` 立即可见新会话行 |
| X6 | 不同渠道发起的轮次标注各自来源 | 通过（桌面 ↔ CLI） | 同一条桌面车道：第一轮在桌面发出，第二轮用 `axon run --session <会话 ID>` 续聊。`inbox` 两行的 `source_channel` 都是 `desktop`，`client` 分别是 `desktop` 与 `cli`；桌面历史同一屏里两个气泡页脚依次是「来源 桌面」「来源 CLI」（CDP 回执：`工作区 \|\| 当前 \|\| 来源 桌面 \|\| 12:20 \|\| 来源 CLI \|\| 12:21 \|\| …`）。更早的索引轮同样带来源：此前只有最新一轮有标注，已在 `indexedTurn` 补齐 |
| X7 | 同一会话跨渠道续聊并混合计数 | 通过（同 X6） | 该会话 `session_metrics` 为 `turn_count=2`、`channel_mix_json={"cli":1,"desktop":1}`；另一条纯 CLI 车道的会话为 `{"cli":1}`。计数键是**逐轮发起端**，同一条桌面车道出现两种来源即证明跨端续聊已按轮计入 |

## TUI（T1 至 T7）

本轮的取证通道：真实控制台驱动（`FreeConsole` → `CreateProcessW(CREATE_NEW_CONSOLE)` → `AttachConsole` →
`ReadConsoleOutputCharacterW` 读渲染后的屏幕缓冲 → `WriteConsoleInputW` 注入按键）。读的是真实屏幕字符而不是
管道字节流，发键走的是控制台输入队列，与被测进程看到手工敲键同一条路径，因此具备 TTY 语义。

| # | 检查 | 结果 | 证据 |
| --- | --- | --- | --- |
| T1 | 启动 | 通过 | 经安装根 `axon.cmd` 起 `cmd /c axon`，屏幕出现 `Axon Personal Agent v0.0.1`、`入口已就绪 · MCP 33 · Skill 32` 与输入坞 `› 描述要完成的任务，或输入 / 选择能力` |
| T2 | 对话 | 通过 | 输入「记住数字 5，只回复两个字：收到」后流式渲染 `◌ 思考 / Thinking · 8.99s` 与回复 `• 收到` |
| T3 | 工具 | 通过 | 输入「用 shell 跑 echo t3-ok 并只回复输出」后渲染工具行 `执行` / `$ echo t3-ok` / `│ t3-ok`，回复 `• t3-ok`；末行给出 `用时 10.79s · Token 53.5K · 首字 2.66s · 速率 149.6 tok/s` |
| T4 | /sessions | 通过 | 输入 `/sessions` 后弹窗列出会话，每条含标题与 `provider · model` 行，可 `↑↓` 选中 |
| T5 | 选中进入 | 通过 | 选中后渲染 `已切换到目标会话，正在加载历史...`，随后按序载入该会话此前的轮次正文与工具证据 |
| T6 | /new 与 /reset | 通过 | `/new` 渲染 `已开启新会话，下一轮使用新的 session id。` 且顶栏读数清零；`/reset` 不改变顶栏读数与会话归属，仅复位本地运行实例（与 `session_commands.rs` 的客户端等价语义一致） |
| T7 | /sessions --all | 通过 | 执行后弹窗标题行显示 `会话 含归档 · ↑↓择 · Enter 入 · Esc 闭`，列表中可见仅存在于归档会话里的条目 |

> 追加约定：T6 的「只换 run_generation」在服务端由 `axon sessions reset <id>` 触达（已实现并有单测）；
> TUI 的 `/reset` 是同一会话上的本地运行实例复位，两者语义在台账 W4-5 行已注明。

## 回放投影一致性（已修复）

真机复验发现回放与实时曾经是两条独立的投影路径，修复后已收敛为一条。以下为当时的缺陷描述与修复后的真机证据。

**缺陷（已消除）**

- **两条独立投影**：实时链路按 wire 事件逐条构造 `user` / `reasoning` / `tool` / `job` / `subagent` / `assistant` / `summary` 各类单元；历史回放一度只按索引拼装 `user`（`history-user:<interactionId>`）与最终文本两类。
- 后果一：打开会话后思考过程、工具调用、作业与子代理卡片全部消失。索引给不出工具名串时，连折叠行都立不起来。
- 后果二：打开会话时先按富形态渲染一帧，回放落地后被瘦形态整体替换，正文内容瞬间塌掉，体感就是整屏闪烁。
- 后果三：标题栏的后台任务清单读的是轮次里的工具单元，折叠释放单元后清单一并消失，重开会话也回不来。

**修法**

- 回放改为**按页读正式 durable 帧**并喂进与实时同一个 `applyFrames`：一页 Interaction 整页升序回放，扫完一轮即按折叠判据折叠，随后整页一次性接进正文（打开只发生一次替换）。索引只提供帧里没有的附件与逐轮来源。
- 删除 `history-user:` 手搓单元与「按索引立起一轮」的旁路，`indexedTurn` 整段移除。
- 后台作业改为会话级台账（`state.jobs`），由帧归约在作业发起处写入，随页回放一起带回来，折叠不再影响它。
- 会话累计耗时不再重复计入：基线里仍标着运行中的轮次由增量承担（`SessionTokenBaseline.openTurnIds`）。
- 契约测试：`tests/transcript-replay.test.mjs`（全量喂入与逐条增量喂入的 `cells` 深相等）、`tests/interaction-replay.test.mjs`（整轮按页读、超预算退化尾窗、代次作废）、`tests/history-plan.test.mjs`（整页回放、增量只接新轮次）、`tests/jobRows.test.mjs`（台账在折叠后仍保留）。

**真机证据（安装态 `versions/0.0.1/axon-desktop.exe`，CDP 9333）**

- 单轮会话冷启动后打开：用户气泡 1、思考行 1、执行行 1、指标行 1；头部 `耗时 6.43s` 与 `session_metrics.total_active_ms=6426` 逐字段一致（修复前读成 16.31s）。
- 三形态与折叠摘要：三轮会话重开后读作「用户气泡 → 过程 · 执行了命令 9.88s / 9.1s → 最终回复」，最新一轮保持展开（思考 511ms、执行 594ms）。
- 展开历史折叠行：点击第 1 轮的 `过程` 后该轮回放出「思考 766ms + 执行 exit 0 · 1.23s + 最终回复 + 指标行」，与实时形态一致。
- 后台任务清单：`清理` 会话里跑过一次后台作业，收起会话再打开后标题栏仍显示 `后台任务 1`（修复前为空）。
- 分页：22 条 Interaction 的车道打开后 `peakTurns=20`、`hasOlder=true`（首屏只取最新一页）。更早一页的点击取证未完成：验证途中安装树被并行开发者的 `install-from-source` 覆盖（`axon.exe` 19:35:52、`axon-desktop.exe` 19:35:17），随后守护进程进入 `goal_id` 空值失败循环、桌面端反复退出，故本条**不作通过结论**。
- 同一页打开耗时实测 15.8s（20 轮，含守护进程故障窗口）。逐轮读帧目前串行，下一步按小并发并行化；本条列为待优化项，不阻塞正确性。

## 按轮分页与逐轮读数（本轮落地）

打开会话改为「一次索引页 + 一轮正文」：索引页（默认 20 轮）当场立起用户气泡、过程抬头与最终回复，随后只把最新一轮读整轮展开；「加载更早」一次向前取一轮。逐轮读数与动作摘要由网关在一次**有界尾部扫描**里给出（扫到本页最早那一轮即停），与实时同一套口径。

**缺陷（已消除）**

- 会话累计恒为全零：`sessionUsage` 绕经 `sessions.run_session_id` 再查 `session_metrics`，而该投影按会话键落行，rollout 退役后这条路径永远查不到；header 的累计与命中率因此整段消失。
- 轮次分割线整行消失：逐轮用时就错在 `inbox.updated_at_ms - received_at_ms`，重构后两列恒等，读数恒为 0，指标行没有一行可渲染。
- 打开会话后最新一轮的过程行与上下文读数丢失：索引骨架自带的收口帧先把该轮判成结束，随后的回放帧另起了一轮，投影取第一轮时把回放结果整段丢掉。
- 索引轮的过程抬头没有动作摘要、分割线夹在过程中间：抬头只有中性的「过程」，而生成中先行立起的指标行会被后到的工具行与正文插到前面。

**修法**

- 会话读数直接读 `session_metrics`（按会话键），累计口径为「输入 + 缓存读 + 缓存写 + 输出 + 推理」。
- 逐轮读数、最终正文与工具名由 `page_facts` 一次有界尾部扫描给出：收口事件划定一轮边界，`turn_metrics` 与 `toolCall` 归属最近一次收口；未覆盖的轮次只用工作项终态时间差补用时。
- 有帧就纯按帧投影（`historyProjection.projectFrames`），无帧才走索引骨架；两路不再叠加。
- 分割线由 `orderTurnCells` 固定排在该轮全部工作之后、待转投消息之前；生成中用量帧一到即立行并随节拍走字。
- 契约测试：`tests/liveTurnSummary.test.mjs`、`tests/summaryOrder.test.mjs`、`tests/sessionGeneration.test.mjs`、`crates/axon-eventlog/tests/contract.rs` 的 `latest_events_are_read_per_interaction_from_the_tail`。

**真机证据（安装态 `versions/0.0.1`，CDP 9333）**

- 打开 23 轮车道（事件日志 1.28 MB，首屏 20 轮）：实测 468 至 781 ms 完成首屏，网关索引页耗时 300 ms（修复前同一接口 888 ms）。
- header 读数：`耗时 2m 35s · 累计 5.48M · 命中率 86.90% · 399K/1M · 首字 4.12s · 速率 48 tok/s`；累计与 `session_metrics` 分桶求和一致，上下文段读作 `399K/1M`。
- 索引轮分割线：`用时 4.91s · Token 84.9K · 入 14.6K · 缓存命中 70.1K · 出 72 · 推理 61 · 命中率 82.74% · 首字 3.14s · 速率 167.1 tok/s`，逐项来自该轮 `turn_metrics`，未回放正文。
- 索引轮折叠摘要：`过程 · 执行了命令 12.23s`、`过程 · 执行了命令并已调用工具 13.54s`、`过程 · 已读取文件并执行了命令 6.89s`（「加载更早」取回的更早一轮）。
- 分割线位置：末轮子节点顺序为「用户 → 过程 → 正文 → 指标行」，指标行是该轮最后一个子节点（`[role=separator]` 在轮内）。
- 「加载更早」：20 → 21 轮，一次一轮，导航条刻度 20 → 21。
- 消息导航条：20 个刻度，点击可定位（`role=navigation`）。
- 生成中打开：提交后重载界面再进会话，header `累计 6M → 8.09M`、`上下文 417K/1M` 持续进位，工作状态行读作 `生成中`，末轮指标行边生成边更新。
- 计划：`update_plan` 轮在生成中 header 出现 `计划 0/3`，正文出现 `更新计划 · 建立诊断任务三步计划 1ms`；重开该会话后计划单元仍在（最新一轮保持展开），而 header 的计划段按设计只在有轮次在跑时占位。
- LSP：该轮的工具行读作 `LSP 诊断类资源`；header 的 LSP 段取实时观测（`activeClients`），本轮未挂上 LSP 客户端（无 rust-analyzer 子进程），因此**未取得** LSP 段的真机证据。

## 环境副作用登记
- 源码安装走 `scripts/install-from-source.ps1 -Components all`（当前唯一合规入口）。脚本会静默提权、重建版本树并拉起一次 Daemon 与桌面端；验证前已全部终止。
- 该次 Daemon 启动会向真实 `$HOME/.axon` 追加运行态写入；已删除其留下的 `data/daemon.pid`，真实 `config.toml` 哈希未变，真实数据未删除。
- 验证期间桌面端以 `--remote-debugging-port=9333` 启动，验证结束必须清掉该环境变量与调试端口。
- X6/X7 取证那次只改了 `crates/axon-desktop/ui/**`，按纯前端快速通道重装：`npm run build` → `cargo build --release --locked -p axon-desktop`（`Finished 'release' profile ... in 2m 07s`）→ 覆盖 `versions/0.0.1/axon-desktop.exe`（12:36:45，与 `target/release` 的 SHA-256 一致，`current.json` 未变）→ 经安装根 `axon.cmd desktop` 重启。任何 Rust 侧改动都不适用这条通道。
- 本轮回放修复同样只改 `crates/axon-desktop/ui/**`，按同一条快速通道重装（`Finished 'release' profile [optimized] target(s) in 1m 41s`，覆盖后 `axon-desktop.exe` 与 `target/release` 的 SHA-256 一致，`current.json` 仍为 `0.0.1`）。
- 临时验证根 `$env:TEMP/axon-verify-rt` 保留供下一轮取证；其中含真实 `.mcp.json` 的副本（含本机 key），收尾时需一并清理。
