# TUI 说明

运行 `axon` 时，将启动全屏交互式终端工作台（Terminal TUI）。系统作为你的专属个人智能体（Personal Agent），无论在终端 TUI、桌面工作台还是无头命令行中，均复用同一套底层 Agent 运行时与状态机模型，不维护第二套会话或工具执行体系。

## 布局

```text
┌──────────────────────────────────────────────────────────────────┐
│ HeaderBar: 模型 · 上下文压力 · 首字/速率 · 计划/LSP 状态         │
├──────────────────────────────────────────────────────────────────┤
│ RenderViewport (独立滚动视口):                                   │
│   • 用户消息气泡                                                 │
│   • 助手流式回答 (Markdown · 语法高亮 · Diff 差异对比)           │
│   • 工具调用状态行 (智能折叠/展开 · 审计回执)                    │
│   • 交互式提问单元 (单选/多选/自定义输入面板)                   │
│   • 会话级指标分割线 (Token Usage · 耗时)                        │
├──────────────────────────────────────────────────────────────────┤
│ ComposerDock (输入与状态坞):                                     │
│   • 多行输入编辑器 (/ 命令 · $ 技能 · @ 文件上下文)              │
│   • 快捷键导航 (Ctrl+R 历史搜索 · Ctrl+X 外部编辑器 · Esc 取消)   │
└──────────────────────────────────────────────────────────────────┘
```

- `HeaderBar`：模型、上下文、审批与运行状态。
- `RenderViewport`：用户消息、Agent 回复、工具、Diff、Reasoning、错误和用量。
- `ComposerDock`：输入、附件、候选、审批与运行中控制。

Header 在上下文信息之后显示会话级 generation 性能指标，随后按需显示计划与 LSP 指标。例如：`首字: 8s · 速率: 40 tok/s · 计划: 1/3 · LSP: 2`。段落因宽度不足换行时仍保持声明顺序；例如速率过长换行后，计划必须跟随换行，不能回填第一行。

Reasoning 流按 provider 的独立思考 block 投射为独立 transcript part；收到 block 结束事件后立即折叠，后续 block 会重新展开。

- `首字`：整个会话内各轮 TTFT 的算术平均：在跑轮的首字帧一到即参与平均，不必等该轮收口。同一轮的观测帧会重复到达（首字帧与收尾帧都带 ttft），按轮身份幂等覆盖，同一轮只计一个样本。
- `速率`：整个会话的加权总和 `Σ(output_tokens + reasoning_output_tokens) / Σ decode_ms`，含思考输出。分子取 Provider 实测的本轮输出：用量帧是**每条流的累计快照**，同一份快照只投一次，按流覆盖而不是逐帧累加。解码窗口是一轮内各条 provider 流的实测流式耗时之和，再减去首字（首字只从第一条流的时间里扣，思考型模型的首字可能晚于第一条流收口）：工具执行、等待与未收口的失败重试都不计入，观测不到任何流边界的轮退回整轮墙钟口径；没测到首字、或 Provider 未返回有效 usage 的轮不进分子分母。每收口一条 provider 流就补一帧实时指标，因此在跑轮在整轮收口之前就参与会话读数，与首字同一种实时性；扣掉首字后窗口不足 250ms 的流整个丢弃（窗口与 token 一起进出），Provider 把思考憋在服务端时第一条流常在首字之后立刻收口，几十毫秒的窗口配一整段 token 会算出几千 tok/s 的假速度。没有任何样本时该段不显示，不渲染 `0`。

桌面工作台与 TUI 恒等：同一份会话级口径、同一套分段文本（含 `速率: 40 tok/s` 的空格与数值后的 `:` 空格）。

Turn 收尾的 token usage 分割线会附带**该轮**的首字与速率（不是会话均值）；该轮速率与头部同口径，分子含思考输出。旧快照缺少这两项时保持原样，不渲染空指标。

用量读数（当前上下文、缓存、命中率与分割线的 token）按「provider 流」记账：一条流的用量帧是**该条流的累计快照**，流内覆盖、流收口时并入累计，只有跨流才相加；只保留最后一条流会漏掉前面各流。流内实时帧与收尾补发的最终用量是同一份快照时只投递一次，上下文构成估算随每一条用量帧投出，收尾帧不再单独重发。

计划指标直接从当前活跃或 transcript 中最新的 Plan cell 反推；Plan cell 不存在时 Header 计划指标同步消失。

## 输入与运行

- `Enter` 提交输入；`Alt+Enter` 或 `Shift+Enter` 插入换行。
- `↑/↓` 在显式换行和软换行形成的视觉行之间移动，并按终端显示宽度保持目标列；单视觉行或到达边界后用于浏览历史输入。
- `Ctrl+R` 搜索历史输入；取消搜索或从历史末尾返回时，会恢复浏览前尚未提交的完整草稿状态。
- `Ctrl+X` 使用 `VISUAL` 或 `EDITOR` 打开外部编辑器；编辑后仍存在的附件占位会继续保留附件绑定。
- 运行中输入进入队列，在安全边界注入当前任务。
- `Esc` 优先处理已提交队列；没有待处理消息时中断当前任务。
- `/` 打开命令候选，`$` 打开 Skill 候选，`@` 打开文件和上下文候选。
- 附件、剪贴板、多行粘贴、历史草稿和会话恢复均由同一输入状态机处理。

所有可见记录来自本地会话与运行时事件；TUI 只渲染结构化 cell，不绕过审批或工具执行边界。

## 用户提问面板

模型把决策交给用户时，输入区上方弹出提问面板；面板打开期间按键全部归面板，输入框此时不接收按键。

- `↑` / `↓` 在选项之间移动并环绕。焦点在自定义答案里时，它们先退出自定义答案，再把焦点交回选项列表，已键入内容保留。
- `空格` 勾选或取消当前聚焦项；单选题为替换，多选题可多选。
- 直接键入字符进入自定义答案，`Backspace` 回退。
- `Enter` 提交当前题：还有下一题就前移，最后一题交出全部作答；单选题在既未勾选也未自定义时提交当前聚焦项。
- `Tab` 显式跳过当前题，编码为空选择集（不是取消提问）。
- `Esc` 在选项列表里收起面板，收起不等于取消：窗口延长或重新进入会话时面板会再出现；在自定义答案里 `Esc` 只退出编辑、面板不收起。
- 面板底部的提示行按相位给键位：选项列表相位写「空格 选择 · 字符 自定义 · Enter 提交 · Tab 跳过 · Esc 收起」，自定义答案相位写「字符 写自定义答案 · ↑↓ 回到选项 · Enter 提交 · Tab 跳过 · Esc 退出编辑」。
- 等待与延长相位里，时间线单元只留题面、状态与「在底部面板作答」的指引，选项列表由面板独家承担；提问挂起或结算、面板收起之后，单元才完整呈现选项与最终作答作为回看事实。

TUI 到 Daemon 的每一次调用都有界：`SubmitInteraction`/`GetInteraction` 往返、以及订阅握手（`SubscribeEvents` → `subscription_ready`）超时即失败返回，Daemon 卡死时不会无限等待。运行中**只有真实连接故障**（读错误、EOF、连不上）才按 cursor 重建；订阅读取超时（静默）不算失去订阅——心跳迟到、长工具调用期间事件流长时间无话都属正常，此时保持原订阅继续等待，既不重连也不计入恢复预算。恢复判据分两层：**订阅活性**（任一条订阅消息，含 `subscription_ready` 与心跳）与 **durable 进度**（新 wire 事件或 Interaction 状态推进），任一推进即视为订阅已恢复；心跳在即即视为订阅健康，durable 进度只用于证明回合在往前走。既拿不到任何订阅消息、又没有 durable 进度并连续 `SUBSCRIPTION_RECOVERY_BUDGET`（55s）时，本轮以携带已积累 `wireEvents` 的结构化错误收口：正文仍由该终态按 durable 顺序全量重放补齐，Daemon 中的 Interaction 继续执行，可用 `axon debug session` 追读，或在 TUI 中恢复该活动回合。运行中超过 5 分钟没有任何事件时仍然只投影一次空闲提示，不取消提交。

TUI watcher 仅对 MCP 配置和 Skills 输入做变更检测：按固定间隔计算用户/工作区 MCP 配置与 Skills 文件指纹，只有指纹发生变化时才主动重载对应 catalog；输入未变化不会发出 MCP/Skills 刷新事件。Workspace instructions、Provider、Session metadata 和 Network 不再纳入 TUI watcher。

`/mcp` 是显式刷新入口：本地强制探测（绕过健康负缓存与 schema 缓存）更新列表显示，同时向 Daemon 提交 `RefreshMcpCapabilities`，由 Daemon 清除健康负缓存、作废并后台重建该 Workspace 的启动快照，使恢复的 MCP 工具在下一轮 turn 进入模型可见工具表。Daemon 不可用时只记告警，本地列表照常刷新。

Windows 系统剪贴板访问在 TUI 统一入口内使用进程级互斥锁，覆盖文本读取、复制和图片读取，保证并发调用的线程安全性。

## 实时投影

Turn 进行中，供应商连接状态、工具调用与回执、文件差异、计划更新、usage 和推理过程按 Daemon durable sequence 实时进入 transcript。终态阶段只追加记忆、技能、治理、运行错误与 canonical assistant 等稳定快照，不重放已完成的工具、计划、差异、推理或 usage。

提交后立即以本轮供应商/模型身份投影“正在连接”，收到真实 `ProviderStatus` 后原位刷新传输细节，并依次推进到连接成功与生成回复。每个成功 Provider 流的 `StreamingCompleted` 在最后一个 delta/usage 之后到达，TUI 先结算 TTFT/速率快照，再处理整轮唯一 terminal `RunLifecycle`。最终 usage 即使只在聚合结果中出现，也由共享 producer 在 completed 前补发。

TUI 不建立第二套事件重排窗口或提前完成门禁：Daemon 已在同一 Workspace lane 内按 durable sequence 发布，消费者按到达顺序投影。canonical assistant 和其他终态快照必须先于 terminal，因此不会提前丢弃最终正文。投影边界只额外强制一条 O(1) 不变式——同一 durable stream 内已投影 sequence 必须严格递增：序号不高于已投影水位的迟到帧直接拒绝并计入 `staleCount`，既不排队也不延迟，契约成立时与逐个投递完全一致。

## 终态投影与 durable 顺序

终态投影不是按 reasoning、assistant、tool、diff、plan、usage 等类别重新分组，而是按 wire 事件的 durable sequence 单遍推进：每条投影行落在其 durable 事实最终成立的位置上，稳定 part id 由 payload 自带。终态重放因此对 reducer 幂等——已由实时流提交的 cell 按 part id 原位替换，未提交的行才追加，重复重放不会产生第二条正文或第二个工具 cell。

`finish_submit` 只做两件事：封口实时流（提交流式 assistant 与 reasoning cell）并按 durable 顺序重放终态投影。这里没有 assistant 补偿、文本级去重，也没有终态淘汰，避免终态补偿与 post-foreground 过滤互相打架。本轮最终 assistant cell 在重放期间先摘出，重放结束后由 canonical 行落位，保证补齐的过程事实始终在最终正文之前。

Provider 重试会撤销当前 generation 尚未提交的 part，终态重放同样不复活它们。

终态只有 wire 一个来源：CLI 不再消费本地 legacy turn event 通道，终态输出全部由 `sessionId`、`wireEvents` 与顶层失败兜底构成。审阅（Review）终态同样按 `ReviewStatus` 的 durable sequence 投影，同一 `part_id` 的多帧快照收敛为最后一次，因此审阅行的位置等于其 durable 位置，不会被固定追加到末尾。

## 邮箱背压

提交级 mailbox 只做四件事，绝不改变语义边界：同一 durable `(stream, event_id)` 的幂等去重、同一 durable stream 内 sequence 单调水位（拒绝迟到帧并计入 `staleCount`）、同一 assistant part 连续 delta 的有界批合并、以及满载时对**可重建展示增量帧**的淘汰。

语义边界帧（工具调用与回执、生命周期、plan、usage、canonical 正文、审批、steer 回执）既不淘汰也不阻塞生产者：展示增量帧被限制在预留语义槽位之后的容量内，溢出时淘汰最早的增量帧并计入 `shedCount`。被淘汰的增量内容会在终态按 durable 顺序完整重放，因此不会形成 durable 语义缺口。任何路径都不会让慢 TUI 反向拉长 Provider 实时路径。

“终态必定是 durable 全量重放”是淘汰策略成立的前提，对失败回合同样生效：宿主必须在提交失败时连同已获得的 durable 终态投影一起收口（`AgentTuiSubmitFailure::with_output`），Daemon 失败报告也必须携带 `wireEvents`。只上报错误文本会让被淘汰的中段增量永久缺失，正文出现无法察觉的缺口。

思考过程在流式阶段展开以便观察；对应 reasoning part 完成或 turn 收口后会自动折叠，仅保留标题与耗时。用户仍可通过原有 transcript 展开操作查看已完成的思考正文。

## 后台终端投影

每次 `ToolCall` 和 `ToolResult` 都作为独立事实保留在会话记录中，确保恢复、审计和幂等语义不变。TUI 展示层按 terminal `session_id` 聚合空的 `write_stdin` 轮询：运行中输出和最终状态原地更新初始 terminal cell，不为每次轮询新增 cell。

包含实际 stdin 内容或中断信号的交互仍独立展示。终端标题只呈现业务命令与状态，隐藏 PowerShell UTF-8 初始化等执行包装；展开详情继续提供有界的高价值输出。

## 工具行的折叠态与展开态

折叠态只给一项事实。规则表是 `axon-display` 的 `tool_row_rules.json`，桌面端、TUI 与飞书工具卡共用同一份：模型自述的 `intent` 优先（声明该字段的工具为 `builtin.shell_command`、`builtin.write_stdin`、`builtin.update_plan`、`builtin.glob`、`builtin.search`、`builtin.notes_write_file`、`builtin.notes_append_to_file` 与 `builtin.send_message`，模型调用时必须给出）；没有 `intent` 时按工具取专用主参数（记忆检索的 `query`、技能的 `skill_name`、子代理的 `task`、`send_message` 的 `message`、`job_output` 的「等待 `<job_id>` 结果」、新建上下文的 `reason`）；MCP 工具按服务名与工具名展示。

规则命中时标题行已经承载主事实，折叠态不再重复参数行：命令原文、原始参数与有界输出只在展开态出现。`get_context_remaining` 的规则是静默，收起时只留工具名。没有专用规则的工具保持既有参数投影（命令原文或主参数值），不猜内容。

## 文件变更投影

每次工具写入仍完整保留 `ToolCall`、`ToolResult` 和 `FileChange` 事实。只要 `FileChange.tool_part_id` 能关联到工具回执，TUI transcript 就以文件 diff cell 作为该次写入的唯一可见投影，不再重复展示同源工具回执；该规则同时适用于内建 Edit 与 MCP 写入，且不会隐藏同轮其他未产生文件变更的工具结果。

Assistant 正文中的 `<oai-mem-citation>` 是内部 Memory 使用信号。canonical response 保留原始标记供 citation 解析与 usage 统计；TUI 的流式、快照、恢复回放和最终 transcript 投影统一隐藏完整块、未闭合块及跨 delta 的半截标签，不把内部协议泄漏到用户可见正文。

Memory usefulness 的 `Memory cited ... adopted ...` 事件仅用于 durable 审计，不投影为用户可见 Hook；Memory recall 的用户可见内容仍按既有可见性规则过滤。

Resume 恢复保留历史消息、工具结果和真实文件变更，但不重新投影历史 Thinking/Reasoning，也不显示没有实际增删内容的 `(+0 -0)` 文件变更；这些事件仍保留在会话快照中，不影响审计事实与恢复后的实时投影。

进入 TUI 时，CLI 先在当前 Workspace 的 TUI 车道中查找仍处于 processing 的 Interaction；若找到，即沿该 Interaction 原有 destination 车道恢复并继续渲染，不要求新 TUI 复用旧车道 ID。若没有进行中的 Interaction，再定位 Workspace 内最近一次 TUI Interaction 对应的 durable 事件流，投影最近已完成 `turn_completed` 的最终 assistant 回复、真实 token usage 分割线，并绑定对应 Session 到 Header；没有历史完成回合或 Daemon/存储暂不可读时，保持普通启动页，不阻断新任务。

`axon run` 复用统一 Daemon durable/realtime 事件订阅链路，按 Interaction 等待并恢复 `turn_completed`。Text 模式会通过同一订阅实时写出 assistant 增量，终态只补未投影尾部；JSON 模式保留完整结构化 wire 事件，TUI 与渠道继续消费完整的过程事件。

## 计划与检查

复杂任务调用 `builtin.update_plan` 后，runtime 会把成功回执投影为 `PlanUpdate` wire 事件。计划 cell 使用 `plan:<session_id>` 作为稳定身份，因此任务开始、阶段完成和验证失败后的更新会原地替换同一 cell，不会重复追加多张计划卡片。

计划状态统一映射到现有 transcript 样式：`pending` 为弱化方框，`in_progress` 为高亮方框，`completed` 为完成标记和删除线。标题显示完成数与总数，说明文本和步骤会随终端宽度重新换行。live progress、最终 JSON、会话 replay 都只消费同一 `PlanUpdate` 结构；`verification` 证据保留在工具回执 metadata 中用于审计，不扩大 TUI 协议面。

## 子代理完成结果

子代理完成时，控制面会向父代理 mailbox 投递 `FINAL_ANSWER` 通知。完成事件同时在 TUI 追加一条终态记录，并移除运行中的子代理活动行。终态标题为“任务名子代理完成”，详情固定按“耗时、Token 消耗、摘要”排序，例如“代码审查子代理完成”“耗时：12.3s，Token 消耗：2.48K，摘要：审查完成”。耗时与 Token 数量复用项目统一的显示单位函数；Token 为 0 或供应商未回传 usage 时，省略整个“Token 消耗”字段。

完成摘要按 Unicode 字符数判断：不超过 4000 个字符的结果直接放入通知；超过 4000 个字符时，运行时会把完整回执以 Markdown 写入统一内容归档 `~/.axon/data/archive/{sha256}.md`，通知中附带归档绝对路径，父代理可按引用读取该文件。归档目录属于运行产物，不纳入版本控制，并按统一保留期自动清理。
