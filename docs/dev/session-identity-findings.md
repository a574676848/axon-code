# 会话身份重构真机取证问题清单（第 1 轮）

> 本文登记会话身份重构（`session-identity-refactor-plan.md`）W8.5 与 W10 各项在**安装态真机**上暴露的问题点。
>
> 状态口径：**已修**表示改动已落源码；**已生效**表示已覆盖安装并在真机复核过；**待修**表示已定位、未改；**待取证**表示尚未真机复核；**待裁决**表示需要长官先定方向。
>
> 证据口径：只写可直接复核的命令、会话标识与回执行。离线单元测试与离线门禁不得冒充真机通过；未取证项一律如实标注。

## 结论摘要

| 项 | 结论 | 依据 |
| --- | --- | --- |
| W10.14 TUI 进程内复用会话 | 通过 | 同一进程两轮落在同一条会话，`turnCount=2`、`channel_mix_json={"tui":2}`，未出现第二条 primary |
| W10.3 CLI 续接 | 通过 | `run --session` 后该会话 `turns 1→2`，Thread 未变，全库会话行数不变 |
| W10.4 不存在 id | 通过（旁证） | `--session` 指向不存在 id 时退出码 2、不产生会话行 |
| W10.5 优先级 | 行为通过，文档曾不一致 | 单次 `--session` 高于 resume 默认并还原指针；`resume` 只喂 `--continue`；裸 `run` 新建 |
| W10.1 TUI `/sessions` | 待取证 | 数据源、条数口径、滚动窗口、回车语义与标题省略号均已修（原 F9 至 F14），未真机复核 |
| W10.2 TUI 选中切换 | 待取证 | 高亮重绘、指标随身份切换、标识口径与冷启动回放均已修（原 F15 至 F18），未真机复核 |
| W8.5 目标执行分支 | 部分生效 | 真机确认执行轮落 `kind='goal'` 会话、逐字稿落 `kind='subagent'` 执行子会话；指标投影缺行见 F22，子会话未绑定目标与执行标识见 F23 |
| W10.7 至 W10.12 桌面项 | 待取证 | 第一轮桌面取证被中断，未留完整证据 |

## 一、数据链路

| 编号 | 严重度 | 问题 | 证据 | 根因位置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| F4 | 高 | 一次 turn 存在两套执行器，指标只在其中一条产出 | 主 turn 走 `run_run_with_progress`；执行子会话走 `ProviderRunSubagentExecutor` | 执行段已合并为 `execute_turn`（provider 循环在 `run_turn_execution`，终态在 `run_turn_terminal`）；装载差异收敛在 `TurnSpec`，运行后差异只剩 `TurnCompletion` | 已修复 |
| F5 | 中 | 新工作区首轮瞬态失败 | `Session Worker 失败：Interaction 存储失败：Query is not read-only`（183ms，复跑三次成功） | 该文案是 rusqlite `InvalidQuery` 的 Display：schema 版本不匹配已改为 `DataIncompatible`；交互路径全部行解码点（InteractionId、来源、JSON 列、会话/动作/宿主标识、kind/trust_scope、状态与数值列）统一走 `decode_error`，对外一句人话、字段与取值进日志与 Debug | 已修待取证 |
| F6 | 中 | `sessions.root_session_id` 标识空间混用 | 同一列既有会话 ULID，也有 Thread UUID（两条不同派生点） | 子代理登记点与自迭代登记点 | 待修 |
| F22 | 高 | 执行子会话的轮次指标只进 `usage_facts`，`session_metrics` 无行 | 真机：`usage_facts` 有 `01M7S13T0J…`、`01M7S132F…` 等子会话各 1 行；`session_metrics` 全库仅 3 行且全为 `kind='primary'`，`kind='goal'` 会话与 `kind='subagent'` 执行子会话均无行（含执行成功的 `goal-4bd82aab…`） | 终态记账已按来源统一到 `account_completed_turn`，各来源的 `metrics` 口径均为真；真机行数未复验 | 待取证 |
| F6b | 中 | 子代理与目标执行的轮次没有自己的入站事实，前端必须为这两类会话另写渲染分支 | 会话清单是 `FROM sessions`，但首条输入、来源渠道、投递目标都读 `inbox`；子会话这些列全为空，标题回落成 cwd | 已收口：领取按 `sessions.kind` 只领 `primary`/`cron`，派发轮在起跑时补写输入事实（内容是本轮交给模型的完整提示，目标执行保留 `client=goal` 归因）；目标执行按主会话口径装载与收口，完成回流改投创建它的主会话 | 已完成 |
| F23 | 中 | 执行子会话未绑定目标与执行标识，状态停在 `created` | 真机：`01M7S0KNSR86BR8AY9YAQ3KWNG` 的 `goal_id`、`execution_id` 均为空，`status='created'`，而 `last_event_sequence=16421`、thread 3.7 MB | 子会话登记点只写 `root_session_id` 与 `agent_path`，未回填 `goal_id`/`execution_id`，轮次状态也没有推进 | 待修 |

## 二、CLI 面

| 编号 | 严重度 | 问题 | 证据 | 根因位置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| F7 | 中 | `sessions resume` 的 `--help` 与用户面文档仍宣称「让后续 run 追加到同一会话」，与实现相反 | 实测：裸 `run` 新建会话；`resume` 只影响 `--continue`；`--session` 单次覆盖并还原指针 | `session_commands/mod.rs` 的 about 文案、`docs/skills/operate-axon/SKILL.md`、`references/capabilities.md`、`references/cli-reference.md`、`docs/dev/storage-architecture.md` 的 C4/C5 | 文档已修；`--help` 待重编生效 |
| F8 | 低 | `sessions resume` 的参数文案宣称同时接受会话 ID 与 Thread ID，实际只收会话 ID | 传真实 Thread ID 直接报错并退出码 2 | `session_commands/thread_cli.rs` 的共享 arg 文案 | 文案已修；`--help` 待重编生效 |

## 三、TUI 面

| 编号 | 严重度 | 问题 | 证据 | 根因位置 | 状态 |
| --- | --- | --- | --- | --- | --- |

## 四、桌面面

| 编号 | 严重度 | 问题 | 证据 | 根因位置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| F21 | 待取证 | W10.7 / W10.8 / W10.9 / W10.10 / W10.11 / W10.12 未取得完整真机证据 | 第一轮桌面取证被中断 | 无 | 待取证 |

## 五、未取证与边界登记

- 数据面真机取证（本次）：安装态 `$HOME/.axon/bin/versions/0.0.1/axon.exe` 与 `target/release/axon.exe` 同为 149060608 字节、同 `2026-11-19 12:19:39` 时间戳，运行中的 daemon（PID 12300）即该版本目录内二进制。在该版本上跑目标执行 `goal-2e3f2a7f88c4c0590f00a8523825e438`，对照来源会话 `01M7S0KNRM9SZ9VZ2X92JDA3NZ`（`kind='goal'`、`last_event_sequence=283`、无 thread 实体）与执行子会话 `01M7S0KNSR86BR8AY9YAQ3KWNG`（`kind='subagent'`、`root_session_id=01M7S0KNRM9SZ9VZ2X92JDA3NZ`、`agent_path=root/task-01m7s0knrm9sz9vz2x92jda3nz`、`last_event_sequence=16421`、thread 3.7 MB）：逐字稿只落执行子会话成立；指标两侧都没有 `session_metrics` 行，见 F22。复核命令：`sqlite3 -readonly $HOME/.axon/data/control.sqlite3` 查 `sessions` 与 `session_metrics`，`sqlite3 -readonly $HOME/.axon/usage/usage.sqlite3` 查 `usage_facts`；`session_events` 不是可查表，逐字稿以 thread jsonl 与 `sessions.last_event_sequence` 为准。
- 历史会话的 `session_metrics` 不会因修复自动补齐；是否按事件重放回填待裁决。
- W10.1 的 F9 至 F11 已修待取证：`/sessions` 改读 `sessions(kind='primary')`，与会话清单同一批行、同一页大小；模型行改读会话行的 `provider` / `model_name`（轮次收口时条件单行写回）。需在安装态用 PTY 取证：列表无内部线程标题、`--all` 含归档并带前缀、条数与 `axon sessions list` 一致、每条带 `provider · model`。
- W10.1 的 F12 至 F14 已修待取证：弹窗按真实面板高度开窗并跟随选中项滚动（`visible_capacity` / `window_start`）；可选参数命令（提示写成 `[--all]`）回车直接执行，只有必填参数命令才先补全；超宽标题与描述按显示宽度裁剪并补 `…`。需在安装态用 PTY 取证：选中末项仍在可视区、`/sessions` 一次回车即打开弹窗、长标题以 `…` 结尾。
- W10.2 的 F15 至 F18 已修待取证：帧签名纳入底部视图选中项（高亮随方向键重绘）；会话身份切换时清空上一条会话的累计/上一轮读数与运行模型，同一会话重复绑定不清空；历史回放的身份行带用户选中的会话标识而不是 thread 标识；冷启动只在显式 `--continue` 或指定会话与回放来源一致时回放。需在安装态用 PTY 取证：方向键后高亮跟随、切换后顶栏读数归属目标会话、切换后 `/status` 显示的标识与会话清单一致、默认启动首屏为空且首轮新建会话。
- 失败轮的子会话终态：运行侧已恢复写入（`failures_settled_by_scheduler` 为假），来源会话的调度终态仍由 Worker 写；两条卡是否会在界面上同时出现待真机确认。
- W10.7 至 W10.12 全部待取证：需要 20 轮以上会话的分页证据、左侧菜单四项与会话表的逐项对照、跨端三轮来源标注、以及目标 / cron / 子代理三个入口的渲染路径比对。
- W8.5：`kind='goal'` 会话与按 `(root_session_id, agent_path)` 固定的执行子会话在真机上已成立（标识位见上条）；`origin_channel=goal` 的渠道分布因 `session_metrics` 缺行暂时无处可看，并入 F22；「重复执行续在同一条子会话」仍未取证。

## 六、复现与取证手段

| 面 | 手段 | 判定 |
| --- | --- | --- |
| TUI | python 3.13 + `pywinpty`（ConPTY）起真实伪终端，`pyte.Screen` 重建屏幕文本；按键走 ConPTY 输入管道，逐字符间隔不小于 0.12 秒 | 屏幕文本片段 + `axon sessions show/read` 与 sqlite 读数三方对照 |
| 桌面 | `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` 后经安装根 `axon.cmd desktop` 启动，`tools/cdp-*.ps1` 驱动 | 只认 `OK:` 前缀输出，`ERR:` 视为未取证 |
| 库侧 | `sqlite3 -readonly $HOME/.axon/data/control.sqlite3` | `sessions` / `session_metrics` / `goals` 三表读数 |
| 日志 | `$HOME/.axon/data/logs/axon-<日期>.log` | `host lease` / `turn_metrics` / 执行失败行 |

取证对象为安装态 `$HOME/.axon/bin/versions/0.0.1`，当日 `axon.exe` 与 `target/release` 逐字节一致；真实用户配置 `$HOME/.axon/config.toml` 的 SHA-256 在取证前后保持不变。
