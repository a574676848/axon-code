# 窗口轮转记录有界化 WBS（方案 A + B）

本文件是「内联窗口轮转被单条 JSONL 记录体量硬上限打死」的整改计划与验收台账。实施按本文件的 WBS 逐项推进，每完成一项登记状态、证据与命令回执。

## 1 背景与失败证据

真机现场（2026-11-21，本机安装态）：

- 会话 `01M7XSBTGE9PHSFXANBQEFXV4F`、Thread `cd7dbcb9-761a-4d1c-bca5-b16b7f7f5ec0` 在 12:01:22 换窗失败：
  `活动窗口投影或摘要来源落库失败，归档和窗口身份已回滚：单条 JSONL 记录超过记录体量硬上限`；
  随后 12:01:29 provider 返回 400 `Input tokens exceed the model context limit`，会话既不能换窗也不能继续。
- 该 Thread 的事件日志共 **41 个分片、916.4 MB、最大单条记录 45.6 MiB**，11:55 至 11:58 之间每 20 秒新增一片。
- 单条 45.6 MiB 记录的构成：`historyItems[].provider_metadata.files[].lines[].text` 21.1 MB、
  `receipt.metadata.files[].lines[].text` 10.5 MB，其余为正文与摘要；单个 `text` 约 3.46 MB，
  内容本身是另一条 Thread 事件 JSON，即检索 receipt 把「整条 Thread 记录」当作命中行原文，
  且在 `receipt.metadata` 与 `provider_metadata` 中各存一份。

## 2 根因

1. **范式缺口**：存储范式要求 append-only 日志「单条记录有界」（`axon-jsonl` 的分片预算 8 MiB、单条硬上限 64 MiB），
   会话事件层已有「正文外置 + 引用 + 缺失降级」的实现，Thread 事件层没有对应机制。
2. **checkpoint 记录天然是 O(会话)**：`compact_thread_from_detail` 写的 `ThreadEvent::ThreadCheckpoint { detail }`
   承载整份 `ThreadDetail`（全部轮次），本会话约 916 MB，任何长会话都会逼近并越过 64 MiB 硬上限。
3. **增长源放大**：检索 receipt 把命中行全文（可能是 3.4 MB 的整条 Thread JSON）双份持久化，
   使单条记录在几十轮内从 KB 级涨到 46 MB，把「长会话才触发」提前成「一小时即触发」。
4. **失败后果**：投影落库失败 → 归档与窗口身份回滚 → 上下文继续超限，换窗永远不成功。

## 3 不变量（本轮要落成的存储事实）

- **INV-1 记录有界**：单条 Thread 事件记录（含 checkpoint）序列化后不超过内联阈值 1 MiB；超出一律外置。
- **INV-2 正文外置**：外置正文使用 `axon_eventlog::encode_frame`（zstd，带校验和），日志行只保留
  `{type:"thread_event_body_archived", digest, bytes, path}` 引用；`path` 相对会话数据根，不含机器相关绝对路径。
- **INV-3 缺失即报错**：正文缺失、摘要不匹配或解析失败必须如实报错，不得静默跳过，历史不可丢。
- **INV-4 一次性升级**：Thread 事件格式随本轮升级到新版本，只保留「内联」与「外置引用」两种形态；
  不保留旧版本解析分支。本机会话数据在安装新版本前整体清空，因此旧记录不需要读得懂。
- **INV-5 检索结果有界**：检索工具出口把单行文本裁到 2 KiB 并标注 `textTruncated`，
  该结果集同时喂给模型可见输出、工具回执与历史快照，因此在出口一次收口；完整内容不复制进结果集，需要时按行号用 `read` 回读源文件。
- **INV-6 三端同源**：CLI、TUI 与桌面端只经 Thread 读取入口（`ThreadService` / `JsonlThreadEventStore`）消费事件；
  所有把 JSONL 行解析成事件的落点（正序解析、尾部逆序扫描、激活快照扫描）必须共用一个解码入口。
  任何一端的读取都不得直接解析 Thread 分片原文，外置记录对三端必须完全透明。

## 4 已裁决事项

| 事项 | 结论 |
| --- | --- |
| 范围 | 方案 A（Thread 事件正文外置）与方案 B（receipt 有界 + 归档引用）都做 |
| 内联阈值 | 1 MiB（分片预算的 1/8，单条硬上限的 1/64） |
| 正文缺失语义 | 读取失败并给出可诊断原因，不做降级跳过 |
| 预览长度 | 命中行 2 KiB 预览 + `textTruncated` 标记；完整正文留在源文件，按行号回读（不写 `archiveRef`） |
| 存量数据 | 清理本机会话数据，不备份；配置、凭据、技能、Cron 任务与用量统计保留 |
| 兼容性 | 不保留旧格式解析：Thread 事件格式一次性升级，靠清空本机会话数据取得干净基线 |
| 执行顺序 | 实施与门禁 → 清空本机会话数据 → 覆盖安装 → 真机验证（新版本不得读到旧格式数据） |
| 方案 D | 「checkpoint 只追加标记 + 独立物理截断」留待后续，不在本轮 |
| 汇报节奏 | 每完成一个 WBS 项汇报一次 |
| 三端衔接 | CLI、TUI、桌面端均须在含外置正文的会话上验证读取与续聊；桌面端换窗后的历史渲染与续聊不得退化为空历史 |

## 5 WBS

| ID | 内容 | 产出 | 验收 | 状态 |
| --- | --- | --- | --- | --- |
| W0 | 拆分 WBS 并落成文档、同步开发文档索引 | 本文件、`docs/dev/README.md` 索引条目 | `verify-docs.ps1` 通过 | 完成 |
| A1 | 新增 Thread 事件正文外置模块 | `axon-session::infrastructure::thread_event_body`：阈值、路径、zstd 写入、digest、还原、目录回收 | 单测 9 条全过（写入/还原/摘要不符/缺失/路径穿越/回收/幂等） | 完成 |
| A2 | 写路径收敛 | `append`、`append_at_expected_sequence`、`replace_thread_with_checkpoint`、`rewrite_thread_with_records` 共用 `encode_record_line`；checkpoint 与管理型重写后按新记录集清理旧正文 | 新增 3 条用例：分片行小于阈值、checkpoint 只剩一条有界记录且旧正文回收、管理型重写只保留被引用正文；原「超分片预算独占分片」「超硬上限拒绝写入」两条旧语义用例按新不变量重写 | 完成 |
| A3 | 读路径收敛 | 新增 `decode_record_line`（内联快路径 + 引用还原 + 缺失/摘要不符原样上抛），`parse_lines`、`read_file`、`scan_all`、激活快照扫描与 `thread_tail` 尾部逆序扫描全部改走它 | 13 条 Thread 存储用例全过；引用记录、尾部读取与 checkpoint 重写读到同一事件序列 | 完成 |
| A4 | 生命周期回收 | `remove_thread` 同步删除正文目录；checkpoint 与管理型重写按新记录集清理旧正文；保留期与会话删除复用同一删除入口 | 删除幂等，删除后正文目录不残留 | 完成 |
| A5 | 测试与集成验证 | 单测 9 条（写入/还原/摘要不符/缺失/路径穿越/回收/幂等/引用形状）；集成 `checkpoint_projection_keeps_records_bounded`（约 12 MiB）与手动用例 `checkpoint_projection_handles_hundreds_of_megabytes`（约 120 MiB） | 两条集成用例均通过；换窗后所有日志记录小于内联阈值、`read_thread` 完整还原；期间发现并修复 frame 明文 64 MiB 上限问题（正文按 4 MiB 分块压帧） | 完成 |
| A6 | 三端衔接回归 | CLI `sessions replay` / `sessions show` 读回换窗后会话；桌面端经 CDP 真实发起一次主动换窗并渲染「上下文轮转 · 窗口 #2」与最终答复；TUI 与两端共用 ThreadService 解码入口（由 A3 覆盖） | CLI 回放 1 轮、桌面渲染正常、无空历史 | 完成 |
| B1 | 检索结果出口收口 | `SearchToolLine` 增加 `truncated`；检索引擎把单行裁到 2 KiB 并补省略号，`match_spans` 同步裁剪；元数据输出 `textTruncated` | 新增 3 条用例：超长行截断并标记、短行原样、元数据带 `textTruncated` | 完成 |
| B2 | 前端与下游适配 | 复核结论：界面不渲染 `metadata.files[].lines[]`，回执摘要与历史快照都从同一结果集派生，无需改前端；契约变化只涉及 `textTruncated` 字段 | 全仓检索确认无 UI 消费者；前端门禁在不改代码的前提下通过 | 完成 |
| B3 | B 的测试 | 后端单测：超长行截断与标记、短行不动、`match_spans` 裁剪、元数据 `textTruncated` | `cargo test -p axon-agent-runtime --lib builtin_search` 8 passed；`agent_run_model_tools` 16 passed | 完成 |
| C1 | 本机会话数据清理 | 按第 6 节清单清理，不备份；安装前执行 | 控制面会话族全 0、`sessions`/`data/sessions` 目录清空 | 完成 |
| C2 | 门禁、构建与真机验证 | 全量门禁、release 构建、绕过 UAC 覆盖 `versions/0.0.1`、模型主动调用 `builtin.new_context` 触发换窗 | 门禁全绿、两二进制哈希一致；CLI 与桌面各触发一次换窗，`window_number=2`、单条记录最大 167 KiB、无换窗失败；验收中发现并修复「同轮可无限换窗」缺陷（每轮最多兑现一次） | 完成 |
| C3 | 提交与推送 | 单一主题提交，中文信息 | 远端同步、工作区干净 | 待开始 |

## 6 数据清理清单（C1，不备份）

清理范围以「会话及其派生数据」为界，以下全部删除：

- `<root>/sessions/<会话 ID>/`：会话便签、窗口归档与运行笔记。
- `<root>/data/sessions/`：Thread 事件分片、Thread 索引、正文归档目录与撕裂尾行临时文件。
- `<root>/data/archive/session-events/`：会话事件正文归档。
- 控制面库（`control.sqlite3`）中的会话族：`sessions`、`inbox`、`work_items`、`work_item_history`、
  `session_metrics`、`turn_activity`、`thread_turn_facts`、`schedules`、`resume_pointer`、
  `goals` 与 `goal_results` 中的测试目标。
- 出站库（`outbox.sqlite3`）中的待答提问与已投递通知。

保留：`config.toml`、身份与凭据、技能、模型与 Provider 配置、Cron 任务定义、记忆与人格数据、
用量统计（`usage.sqlite3` 为跨会话历史口径，不随会话删除）。

## 7 风险与回退

- **格式升级**：新格式不兼容旧记录，且不保留旧解析分支；本机会话数据在安装新版本前清空，因此基线干净。
  若未来在不清空数据的机器上升级，需要另做一次性迁移，不能直接沿用本次策略。
- **写放大**：外置只发生在超阈值记录上，按当前实测每轮最多一条，写放大可接受。
- **B 的展示变化**：命中行由全文改为预览，界面必须给出明确的截断标记与展开入口，避免读者误以为文件内容缺失。
- **清理不可逆**：C1 无备份，执行前先停 Daemon 与桌面端，执行后按第 6 节逐项复核。
- **三端分叉**：若任一入口绕过统一解码（例如自行读分片做回放），外置记录会在该入口表现为「未知事件」；
  A3 与 A6 就是为了把这类分叉在门禁内消掉，而不是等真机暴露。

## 8 汇报口径

每完成一个 WBS 项，汇报以下四项：改动文件、测试或命令回执、验收结论、遗留项。未取证项如实标注，不用测试代替真机结论。
## 9 后续增补：换窗后的记忆连续性（P1 / P2 / P4）

真机验收暴露的问题不是记录体量，而是「换窗后模型只记得一句结论」。三项增补如下：

| 项 | 内容 | 验收 |
| --- | --- | --- |
| P1 | 预算预警与耗尽提示改为固定交接小节：目标 / 已完成步骤（文件路径、命令、关键 ID）/ 待办 / 已验证证据 / 下一步命令，并给一段正例 | `handoff_prompt_requires_the_checklist_and_names_the_history_tools`、`warn_note_also_asks_for_the_checklist` |
| P2 | 换窗时由运行时从 `turn_events` 取最近 1–2 个已完成工具调用，生成有界 `rollover-digest` 运行时提示注入新窗口（单行 160 字符、总量 1.2 KiB；进行中的调用不写入） | `rollover_digest_keeps_the_last_two_completed_tools`、`rollover_digest_truncates_long_summaries`、`fresh_window_injects_the_rollover_digest_note` |
| P4 | 不新增工具：提示语直接给出既有回读链路 `history_list_windows` / `history_search_contents` / `history_read_item`，并在 `rollover-digest` 里再写一次 | 上述提示测试断言三个工具名出现在提示中 |

另外修复：模型主动请求换窗（`builtin.new_context`）原先可在一个 turn 内被无限兑现，实机两分钟换了 42 个窗口；现改为每 turn 最多兑现一次，后续请求只走自动策略。

**真机验收（2026-11-21）**：安装态下让模型自行触发换窗——先 `builtin.read_file` 读 `Cargo.toml` 前 3 行，再只写一句「进行中」便签，然后 `builtin.new_context`。换窗后模型准确复述了三次工具调用、读取的文件与前三行内容，证明信息来自运行时回执而不是便签。落盘证据：checkpoint 的 `compactedHistory.historyJson` 依次为 `conversationCheckpoint`、`{runtimeNote: rolloverDigest}`、`injectedContext…`；`archived_history` 只有 `window_1`（每轮最多一次）。
