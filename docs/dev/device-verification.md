# v0.0.1 真机验收记录

> 验证对象：安装态版本树 `$HOME/.axon/bin/versions/0.0.1/`，与 `target/release/` 逐字节核对一致
> （本轮 `axon.exe` 与 `axon-desktop.exe` 均 hash-match，安装走 `scripts/install-from-source.ps1 -Components all`）。
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
| B7 | 24 小时长跑 | 未真机验证 | 未执行 24 小时观测 |

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

未真机验证。本环境没有交互式终端（无 TTY），自动化通道也不成立：Windows Terminal 的 UIA 树不暴露终端文本，
`agent-desktop` 注入的按键对 ConPTY 无效（注入前后截图 SHA 相同）。离线单测（`/sessions` 弹窗、`/new` 与
`/reset` 拆分）已全绿，但不能替代真机证据。

请在有交互终端的机器上按下列清单逐项执行，并把**输出文本**贴回（不看退出码）：

```text
1) 启动：在仓库目录执行  axon  —— 期待进入交互界面，状态栏正常显示
2) 对话：输入「记住数字 5，只回复两个字：收到」—— 期待流式渲染出「收到」
3) 工具：输入「用 shell 跑 echo t3-ok 并只回复输出」—— 期待工具行渲染命令与输出 t3-ok
4) /sessions：输入 /sessions —— 期待弹窗列出会话，↑↓ 可选中
5) 选中进入：在弹窗里选中一条会话回车 —— 期待历史正文加载正确（含此前手机/桌面发过的内容）
6) /new 与 /reset：分别执行 —— 期待 /new 后顶栏会话标识变化，/reset 后会话标识不变、运行实例复位
7) /sessions --all：执行 —— 期待归档会话在列表里可见
```

> 追加约定：T6 的「只换 run_generation」在服务端由 `axon sessions reset <id>` 触达（已实现并有单测）；
> TUI 的 `/reset` 是同一会话上的本地运行实例复位，两者语义在台账 W4-5 行已注明。

## 环境副作用登记

- 源码安装走 `scripts/install-from-source.ps1 -Components all`（当前唯一合规入口）。脚本会静默提权、重建版本树并拉起一次 Daemon 与桌面端；验证前已全部终止。
- 该次 Daemon 启动会向真实 `$HOME/.axon` 追加运行态写入；已删除其留下的 `data/daemon.pid`，真实 `config.toml` 哈希未变，真实数据未删除。
- 验证期间桌面端以 `--remote-debugging-port=9333` 启动，验证结束必须清掉该环境变量与调试端口。
- X6/X7 取证那次只改了 `crates/axon-desktop/ui/**`，按纯前端快速通道重装：`npm run build` → `cargo build --release --locked -p axon-desktop`（`Finished 'release' profile ... in 2m 07s`）→ 覆盖 `versions/0.0.1/axon-desktop.exe`（12:36:45，与 `target/release` 的 SHA-256 一致，`current.json` 未变）→ 经安装根 `axon.cmd desktop` 重启。任何 Rust 侧改动都不适用这条通道。
- 临时验证根 `$env:TEMP/axon-verify-rt` 保留供下一轮取证；其中含真实 `.mcp.json` 的副本（含本机 key），收尾时需一并清理。
