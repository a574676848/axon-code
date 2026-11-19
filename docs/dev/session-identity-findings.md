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
| W10.1 TUI `/sessions` | 不通过 | 见 F9 至 F14 |
| W10.2 TUI 选中切换 | 部分不通过 | 正文切换与零写入通过；高亮重绘、指标跟随不通过（F15 至 F18） |
| W8.5 目标执行分支 | 不可判定 | 修复执行路径后仅跑通一次；渠道与历史连续待复核（F1） |
| W10.7 至 W10.12 桌面项 | 待取证 | 第一轮桌面取证被中断，未留完整证据 |

## 一、数据链路

| 编号 | 严重度 | 问题 | 证据 | 根因位置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| F1 | 阻断 | ULID 会话标识大写未收敛成合法 AgentPath 段，后台执行整轮被拒 | 目标执行返回 `Runtime(InvalidState("agent_control:invalid_path"))`，该 goal 落 `status=failed`；修复后同一探针 `goal_start` 转 `completed` | `crates/axon-cli/src/run_commands/run_provider_turn.rs` 的 `background_task_path`：会话标识只做小写归一，空标识与非法字符直接报错，失败文本带会话标识 | 已修、已生效 |
| F2 | 高 | 主 turn 的 `turn_metrics` 落库依赖运行态 host 租约，daemon 持租约时 CLI 直接跳过 | 日志当日 10 次以上 `CLI 未能取得 host lease，跳过 turn_metrics 写入` | `crates/axon-cli/src/run_commands/mod.rs` 的 `publish_turn_metrics` 与 `claim_host` 的单活跃 host 判定 | 待修 |
| F3 | 高 | 后台执行 turn 从不落 `turn_metrics`，`session_metrics` 缺行 | 目标执行会话（`kind='goal'`、completed）零行；24 条 `kind='subagent'` 会话零行；桌面 header 因此缺「耗时 / 累计 / 命中率」，首字与速率同源缺失 | `run_provider_turn.rs` 的 `action_kind != PRIMARY_TURN_ACTION_KIND` 分支走 `execute_standalone_subagent`，该执行器不产出终态指标 | 待修 |
| F4 | 高 | 一次 turn 存在两套执行器，指标只在其中一条产出 | 主 turn 走 `execute_provider_command` + 终态批次；后台 turn 走 `execute_standalone_subagent` | 同上 | 待修（方向：统一管线，长官已裁决） |
| F5 | 中 | 新工作区首轮瞬态失败 | `Session Worker 失败：Interaction 存储失败：Query is not read-only`，183ms 失败；同形态复跑三次均成功，根因未定位 | 未定位 | 待取证 |
| F6 | 中 | `sessions.root_session_id` 标识空间混用 | 同一列既有会话 ULID，也有 Thread UUID（两条不同派生点） | 子代理登记点与自迭代登记点 | 待修 |

## 二、CLI 面

| 编号 | 严重度 | 问题 | 证据 | 根因位置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| F7 | 中 | `sessions resume` 的 `--help` 与用户面文档仍宣称「让后续 run 追加到同一会话」，与实现相反 | 实测：裸 `run` 新建会话；`resume` 只影响 `--continue`；`--session` 单次覆盖并还原指针 | `session_commands/mod.rs` 的 about 文案、`docs/skills/operate-axon/SKILL.md`、`references/capabilities.md`、`references/cli-reference.md`、`docs/dev/storage-architecture.md` 的 C4/C5 | 文档已修；`--help` 待重编生效 |
| F8 | 低 | `sessions resume` 的参数文案宣称同时接受会话 ID 与 Thread ID，实际只收会话 ID | 传真实 Thread ID 直接报错并退出码 2 | `session_commands/thread_cli.rs` 的共享 arg 文案 | 文案已修；`--help` 待重编生效 |

## 三、TUI 面

| 编号 | 严重度 | 问题 | 证据 | 根因位置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| F9 | 高 | `/sessions` 数据源是 JSONL thread 目录，不是 `sessions(kind='primary')`，把内部线程混进用户会话列表 | 列表出现「评估以下有界事实包：」「后台 Goal 阶段结束」与 UUID 标题 | `axon-cli/src/run_commands/interactive_tui/catalog.rs` 的 `load_tui_session_items` | 待修 |
| F10 | 中 | 归档口径失效，`--all` 形同虚设 | thread 派生索引归档 0 条，而 `sessions.archived=1` 有 6 条；列表无「已归档 · 」前缀 | 同上 | 待修 |
| F11 | 中 | 条数与任何口径都对不上 | 目录上限 12、屏幕只画 9 个标题（8 条带 `provider · model`）、`axon sessions list` 是 5 条（`--all` 11 条） | 弹窗容量、目录上限与列表投影三处口径不一 | 待修 |
| F12 | 中 | 弹窗无滚动，选中项可能完全不可见 | 内部选中可达 index 11，屏幕只可见前 9 条 | `axon-tui-runtime` 的会话弹窗高度计算 | 待修 |
| F13 | 中 | Enter 被命令建议弹窗劫持，补全后输入坞不重绘 | 输入 `/sessions` 第一次 Enter 不执行（补成 `/sessions --all`），第二次才执行 | 命令补全与输入提交的键位分流 | 待修 |
| F14 | 低 | 超宽标题硬截断且无省略号 | 屏幕显示 `仅复盘随附的 turn outcome 与 evidence r` | 会话行标题渲染 | 待修 |
| F15 | 高 | 选中高亮只在第一次移动时重绘，屏幕高亮与实际选中不一致 | 内部选中到第 4/5 条、屏幕高亮仍钉在第 2 行；Enter 载入的是第 4/5 条 | 帧重绘签名未包含会话弹窗选中项 | 待修 |
| F16 | 高 | 切换会话后头部指标不跟随 | 正文与身份切到目标会话，累计 token / 上下文 / 命中率 / 目录仍是切换前那条 | 切换路径只重建历史与身份行，未重绑用量与摘要投影 | 待修 |
| F17 | 低 | 会话标识展示口径不一致 | 轮内 `/status` 显示 ULID 会话 id，切换后显示 Thread UUID | 切换后回填的标识来源不同 | 待修 |
| F18 | 中 | 冷启动回放上一条会话的正文与读数，但首次提交新建另一条会话 | 未提交任何轮次时屏幕已有 `累计 117K` 与上一条会话正文；首轮提交后落到新会话 | 启动回放与「进程内首轮新建」裁决冲突 | 待修 |

## 四、桌面面

| 编号 | 严重度 | 问题 | 证据 | 根因位置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| F19 | 高 | 目标看板进入的 goal 会话详情，header 缺「累计耗时 / 累计消耗 / 缓存命中率」，速率与首字同缺 | 真机截图；该会话 `session_metrics` 零行 | 同 F3（数据面缺投影），前端按「取不到基线不渲染该段」的既有规则如实缺省 | 待修（依赖 F3） |
| F20 | 中 | 探针 goal 的执行代理越界修改仓库源码 | 执行期间自行改写 `run_provider_turn.rs` 与其测试文件 | 目标执行提示词未约束「禁止修改文件」 | 待修（探针侧已加约束） |
| F21 | 待取证 | W10.7 / W10.8 / W10.9 / W10.10 / W10.11 / W10.12 未取得完整真机证据 | 第一轮桌面取证被中断 | 无 | 待取证 |

## 五、未取证与边界登记

- W10.7 至 W10.12 全部待取证：需要 20 轮以上会话的分页证据、左侧菜单四项与会话表的逐项对照、跨端三轮来源标注、以及目标 / cron / 子代理三个入口的渲染路径比对。
- W8.5 只完成「目标执行能跑通并落在 `kind='goal'` 会话」这一层；`origin_channel=goal`、重复执行落同一条会话（历史连续）、渠道分布投影三项待复核。
- 历史会话的 `session_metrics` 不会因修复自动补齐；是否按事件重放回填待裁决。

## 六、复现与取证手段

| 面 | 手段 | 判定 |
| --- | --- | --- |
| TUI | python 3.13 + `pywinpty`（ConPTY）起真实伪终端，`pyte.Screen` 重建屏幕文本；按键走 ConPTY 输入管道，逐字符间隔不小于 0.12 秒 | 屏幕文本片段 + `axon sessions show/read` 与 sqlite 读数三方对照 |
| 桌面 | `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` 后经安装根 `axon.cmd desktop` 启动，`tools/cdp-*.ps1` 驱动 | 只认 `OK:` 前缀输出，`ERR:` 视为未取证 |
| 库侧 | `sqlite3 -readonly $HOME/.axon/data/control.sqlite3` | `sessions` / `session_metrics` / `goals` 三表读数 |
| 日志 | `$HOME/.axon/data/logs/axon-<日期>.log` | `host lease` / `turn_metrics` / 执行失败行 |

取证对象为安装态 `$HOME/.axon/bin/versions/0.0.1`，当日 `axon.exe` 与 `target/release` 逐字节一致；真实用户配置 `$HOME/.axon/config.toml` 的 SHA-256 在取证前后保持不变。
