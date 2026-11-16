# 记忆与反思子系统重构设计

本文件是 Axon 记忆反思链路（post-run 自我迭代中的 `memory_update` 线）的单一事实源。链路的触发、证据、准入、动作与可见性一律以本文为准；实现与本文冲突时改实现，不改本文，除非先改本文。

记忆存储与检索口径以 [记忆管理系统](./memory-management.md) 为准，人格状态机以 [人格系统](./persona.md) 为准，内置工具字段以 [内置工具契约](./tool-schema.md) 为准；本文不重复定义，只做增量与差异说明。第 9 章给出分期与实现状态，未落地部分按该章推进。

---

## 1. 现状基线与四层缺陷

### 1.1 实测基线

本节与下文对「现状」的描述，以重构前的主干行为为基线；工作区中已存在的分档与提示中间态改动，在 R3 统一收敛，不构成目标态的一部分。

以本机 `memory_runtime_quality` 的 turn 级计数为口径（截至本文撰写时的一次采样，样本为 post-run 全量轮次）：

| 指标 | 数值 | 含义 |
|---|---|---|
| post-run 轮次 | 524 | 触发过 post-run 自我迭代的轮次总数 |
| 语义评估完成 | 440 | 轻量评估器成功返回并通过校验 |
| 反思尝试 | 78 | 真正派生出 `memory_update` 子代理的轮次 |
| 反思产出 | 52 | 子代理至少写入 1 条记忆的轮次 |
| 尝试数为 0 | 446 | 评估判定 `shouldReflect=false` 或信号未达门槛 |
| 尝试 1 次但产出 0 | 26 | 反思跑了但没写 |
| 评估输出被判无效 | 68 | 未通过输出校验，整轮丢弃 |
| 评估未完成 | 84 | 评估器调用失败或超时 |

这组数字说明两件事：反思线在跑，且确实写入过记忆（52 轮，产出的记录在库可查）；但从 524 轮进来、到 52 轮产出，中间损耗超过九成。而用户在界面上完全看不到这些产出，感知就成了「一条都没有」。

2026-11-13 复核采样（R1 落地后，直读本机 `~/.axon/data/memory/memory.db` 的 `memory_runtime_quality` 全表）：

| 指标 | 数值 | 口径 |
|---|---|---|
| post-run 轮次 | 555 | 表行数 |
| 语义评估完成 | 471 | `semantic_assessments > 0` 的轮次 |
| 反思尝试 | 87 | `reflection_attempts = 1` 的轮次 |
| 反思产出 | 61 | `reflection_yields = 1` 的轮次 |
| 尝试数为 0 | 468 | 555 减 87 |
| 尝试 1 次但产出 0 | 26 | 87 减 61 |
| 评估输出被判无效 | 69 | `false_memory_rejections` 求和 |
| 评估未完成 | 84 | 555 减 471 |

与上表同向：反思线在跑，产出轮次从 52 升到 61，新增的 31 轮全部落在「尝试且产出」一侧。损耗仍集中在「评估判否」与「评估未完成」两段，这两段是 R3 与 R4 的靶心。数字随会话累积漂移，判定链路健康只看同向趋势，不认单点阈值。

### 1.2 第一层：触发层信息不足

`assess_semantic_salience` 每轮（仅 root 代理）走一次 `FastDelegate` 模型，工具关闭（`tools_enabled=false`），只吃 `TurnOutcomeEnvelope`：

```rust
pub struct TurnOutcomeEnvelope {
    pub conversation_id: String,
    pub turn_id: String,
    pub agent_path: String,
    pub user_intent_summary: String,
    pub assistant_outcome_summary: String,
    pub user_message_ref: String,
    pub execution_evidence: ExecutionEvidenceIndex,
    pub selected_skill: Option<SelectedSkillExecution>,
    pub final_status: TurnFinalStatus,
}
```

其中 `ExecutionEvidenceRecord` 只带 `evidence_ref`、`tool_name`、`status`、`kind`、`runtime_verified`、`verification_refs`。也就是说评估器能知道「调用过 `builtin.shell_command` 且运行时确认已执行」，但拿不到命令内容、参数、输出与退出码。

评价「这轮有没有值得长期留存的东西」需要判断的是结论，而结论藏在工具输入输出里。只给「有没有执行过」这种布尔信号，评估器只能保守判否。446 轮 `attempts=0` 的主因在此，不在模型能力。

### 1.3 第二层：证据层是预压缩摘要

反思子代理的任务消息由 `bounded_task_message` 拼出：

```text
<档位前缀任务说明>

以下 JSON 是运行时构建的有界事实包，禁止补造其中不存在的执行事实：
{"turnOutcome": {...}, "semanticAssessment": {...}, "reflectionTier": "..."}
```

同时 `inherits_model_tools_history=false` 映射为 `SubagentForkTurns::None`，子代理不继承任何对话历史。

于是反思代理手里的全部材料就是一份摘要：用户意图摘要、助手结果摘要、一串没有正文的 evidence ref。它在 30 秒预算内要么凭摘要推断（容易写成泛泛而谈的空话），要么自己发起检索（实测 47 次执行里 25 次 shell、4 次 memory_write），但摘要里没有文件名、命令、符号这类可检索抓手，能查到什么全凭运气。

窗口加长解决不了这个问题：可读长度不等于可用长度，摘要丢掉的不是字数，是因果链。

### 1.4 第三层：漏斗是硬阈值二值判定

准入只有两档结论：`qualifies_for_reflection` 为真则完整反思，为假则整轮丢弃。真值条件是「按候选类型分别满足若干硬阈值，且存在与该类型匹配的运行时证据」。

叠加的拒收分支还有一组：`is_valid_for` 里 schema 版本、`reason` 非空、七维信号取值范围、候选类型去重、`evidence_refs` 去重、`user_affect` 取值、`user_fatigue` 取值、漂移原因与风格候选长度、漂移是否超限、`evidence_refs` 必须全部落在 envelope 允许集合内。任一不满足即整轮作废（68 轮即由此产生）。

单点否决的代价是：证据已经达标、只是某一维信号偏低的高价值轮次，和彻底无价值的闲聊轮次被同等对待。也没有任何机制把「这轮不够格」的碎片攒起来，等攒够再判一次。

### 1.5 第四层：可见层被三处遮蔽

**其一，界面提示恒为否。** `SelfIterationRuntime::plan()` 里：

```rust
let memory_updated_info_visible = input.subagent_results.iter().any(|result| {
    result.hook == SelfIterationHookKind::MemoryUpdate
        && result.status == SelfIterationSubagentStatus::CompletedWithWrite
});
```

而 `subagent_results` 在规划期恒为空集合（`self_iteration_input` 构造时为 `Vec::new()`），规划完成之后才执行子代理。于是这个字段在设计上永远为 `false`，与真实写入结果无关。

**其二，来源标识没有反思维度。** 反思写入的记录，`source_origin` 按记忆类型映射成 `agentKnowledge`、`agentProcedure` 之类；`source_session_id` 记的是父会话而非子代理会话。按「反思」维度反查得到 0 条，是最容易误判成「链路没跑」的坑。

**其三，审计回执从未落盘。** `SelfIterationReceiptStoreRuntime::append_record` 在生产链路上的唯一调用点挂在 `MemoryAutoSchedulerRuntime::schedule` 内部，而 `MemoryAutoSchedulerRuntime` 除定义、导出与自身测试外没有任何生产调用点。因此运行态目录下从来没有 `self_iteration_receipts.jsonl`，反思全程无回放凭据。

**其四，写入通道与主代理同质。** 本机 `memory_pipeline_jobs` 表里全部 211 条任务的 `kind` 只有 `explicit_write` 一类：反思经 `builtin.memory_write` 工具落库，与主代理手写记忆共用同一条通道、同一套字段，没有任何可区分标记。因此「哪些记忆是反思自动攒的」这个问题在数据层无解，只能靠回执 `source_trace` 前缀事后对齐。

四处叠加的效果是：记忆确实在写，但用户看不到提示、按来源查不到、没有审计日志可回溯、也无法从写入通道反推。这不是感知偏差，是可见性设计缺失。

### 1.6 技能反思已整体移除

反思子系统历史上存在过两条与「技能」相关的轨道，都只产出或修改 `SKILL.md`，不写记忆条目：

| 轨道 | 内容 | 状态 |
|---|---|---|
| 技能反思 | 读取既有 `SKILL.md`，按反思结论改写正文 | 此前已整体删除，源码无残留 |
| 技能进化 | 从 `memory_update` 的反思摘要派生 `learned-procedure-*` 草案，写入 `skill-evolution/state.json` 与 `drafts/`，再由本机命令人工晋升 | 2026-11-13 整体删除 |

技能进化被移除的依据（本机实测，不是推断）：

- 状态文件里累计记录 20 余条草案，绝大多数停在 `rejected`（风险门禁未过），只有 3 条走到 `validated`；这 3 条从未被晋升，`.axon/skills/` 下没有对应目录。也就是说该轨道从未产出过一个真正生效的技能。
- 它与记忆反思混在同一段代码里，是「反思到底产出了什么」长期说不清的直接原因：一次反思可能只产出一份躺在 drafts 目录里没人看的草案，而质量表把这一轮也记成 `reflection_yields = 1`。
- 该轨道全程不写记忆，与本文定义的反思产出（`memory_write` 落库条数）无关，删除它不影响本文任何一条不变量。

删除范围：`memory_skill_evolution` 领域模型、`run_skill_evolution` 与其 materialize、store、测试模块、`axon memory skill-evolution-status|promote` 两个子命令、wire 层的 `SkillStatus` 产生点，以及 `CompletedRun` 与反思报告上的 `skill_evolution` 字段。运行时代码不再有第二条「反思产出」通道。

保留未动：`WireEventKind::SkillStatus` 协议定义与 TUI 侧的接收投影。它们描述的是「技能状态」这类事件的一般形态，不是这条轨道的专属产物，删掉会破坏协议兼容性。

---


## 2. 目标与不变量

### 2.1 目标

1. 反思的输入从压缩摘要换成「索引卡 + 按需检索」，做到原文零截断。
2. 触发从每轮二值判决换成候选池攒批，显著度累积越线才跑。
3. 记忆动作收敛为三态：新增、淘汰、归档。
4. 人格线从记忆线彻底拆开：人格每轮跑、记忆攒批跑，输出域互不污染。
5. 每一次反思产出都可被三条路径看到：界面提示、来源可筛、回执可查。

### 2.2 不变量

以下六条是硬约束，任何分期实现都不得违反：

| 编号 | 不变量 | 校验方式 |
|---|---|---|
| I1 | 记忆只追加，不原地改写正文 | 写入路径只产生新 `memory_id`；旧记录只能 supersede、tombstone 或 archive |
| I2 | 每条反思产物至少 cite 一个有效证据 | `evidence_refs` 非空，且每个 ref 在运行时证据索引中 `runtime_verified=true`，或等于 `user_message_ref` |
| I3 | 反思不落原文 | 与回执 store 的既有门禁一致：`prompt_text_persisted=false`、`raw_transcript_persisted=false` |
| I4 | 输出域隔离 | 记忆线不写 `persona_state`；人格线不调 `builtin.memory_write` |
| I5 | 优先零 schema 变更 | 三态动作复用现有 `MemoryState`、`tombstone`、`archived_at_ms`、`memory_supersessions`、`supersedes_memory_ids` |
| I6 | 失败不遮蔽父任务 | 反思任何环节失败只映射为 `FailedNonFatal`，父任务终态不受影响 |

---

## 3. 三条线架构

### 3.1 分解

```text
turn 收口
  │
  ├─ 线 A 证据线（同步，纯运行时，零模型，目标 <5 ms）
  │    采集 TurnTrace + 生成 ReflectionCard，卡片入候选池
  │
  ├─ 线 C 人格线（并发，FastDelegate，无工具，预算 8 s）
  │    输入 TurnTrace.signals + 有界语义；输出 persona_state
  │
  └─ 候选池判定（纯函数，零 IO）
       ├─ 未越线：卡片留池，本轮不跑反思
       └─ 越线：线 B 记忆线（异步，主模型，带工具，预算 60 s）
             卡片 → 按需 pull 原文 → 生成候选与动作 → 准入 → 写入 + 回执
```

三条线共享同一个输入 `TurnTrace`，不共享输出，也不互相等待。人格线不因记忆线攒批而变成「批次级」；记忆线不因人格线每轮跑而被迫每轮跑。

### 3.2 为什么是三条而不是两条

人格状态的正确性依赖轮次级观测：温度偏移、熟悉度、连续失败计数、待纠偏提示都是逐轮累加的。如果把人格结算塞进攒批的记忆线，最坏情况下 30 分钟内多轮只结算一次，连续失败会被合并、疲劳趋势会被抹平。

反过来，记忆的价值恰恰需要跨轮聚合：单轮往往只是半个线索，攒到几条才构成可复用结论。两条线的正确频率天然相反，因而必须拆开。

---

## 4. 数据结构

### 4.1 TurnTrace（线 A 产物）

```rust
pub struct TurnTrace {
    pub trace_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub agent_path: String,
    pub recorded_at_ms: u64,
    pub user: MessageRef,
    pub assistant: MessageRef,
    pub tool_receipts: Vec<ToolReceiptRef>,
    pub final_status: TurnFinalStatus,
    pub signals: ExplicitSignals,
}

pub struct MessageRef {
    pub message_ref: String,
    pub chars: u32,
    /// 正文摘要指纹，用于去重与漂移比对，不可逆。
    pub digest: String,
}

pub struct ToolReceiptRef {
    pub receipt_ref: String,
    pub tool_name: String,
    pub status: ExecutionEvidenceStatus,
    pub runtime_verified: bool,
    /// 参数与输出的指纹与长度，正文不入 Trace。
    pub args_digest: String,
    pub output_digest: String,
    pub byte_len: u32,
}
```

设计要点：

- `tool_receipts` 只存引用与指纹，正文不在轨迹里。轮次记录（`rollout_turns.record_json`）保存工具回执的 `summary` 与 `output_ref` 指针，输出原文在 `output_ref` 指向的归档文件里；回取走 `builtin.turn_trace` 读归档。Trace 本身有界，可安全随卡片流转。
- `receipt_ref` 必须做到可点查回原文，这是「零截断」的唯一保障。
- `digest` 不可逆，只用于去重、相似度与漂移比对，不承载语义。

### 4.2 ReflectionCard（线 B 唯一输入）

卡片是索引，不是摘要：

```rust
pub struct ReflectionCard {
    pub card_id: String,
    pub trace_id: String,
    pub segment_index: u16,
    pub segment_count: u16,
    pub when: WhenBlock,        // 起始时间、持续时长
    pub location: WhereBlock,   // cwd 引用、Workspace 类别
    pub what: WhatBlock,        // 意图与结果的长度与指纹，不含原文
    pub evidence_range: EvidenceRange,       // O(1) 区间指针，不随调用数增长
    pub top_evidence: Vec<EvidencePointer>,  // 代表证据，固定上限 8 条
    pub signals: SalienceVector,
    pub batch: BatchBlock,      // 窗口起止、卡片数、累计显著度
    pub hints: Vec<String>,     // 文件、符号、命令名等检索抓手
}

pub struct EvidenceRange {
    pub first_ref: String,
    pub last_ref: String,
    pub count: u32,
    pub verified_count: u32,
    pub rejected_count: u32,
}
```

关键约定：

- 卡片尺寸与任务长度解耦。`evidence_range` 是 O(1) 指针，长轮次不靠加长卡片承载证据，靠 `builtin.turn_trace(trace_id)` 分页回取。原定 2048 字节装不下长任务：一条证据指针约 70 至 100 字节，200 次工具调用就是 14 至 20 KB。
- 单卡硬上限 4096 字节。超限按「先清 `hints`、再把 `top_evidence` 按 verified 优先与显著度降序裁到 4 条」两步收敛；`what` 块是否进一步只留指纹尚未实现；`evidence_range` 永不裁剪，计数取真实值，不得因裁剪失真。
- 批内总量上限 8192 字节（约 2000 至 2500 token），常态目标每卡 800 至 1200 字节，4096 只是防御上限。这个量级对齐对子代理返回摘要的公开观察：子代理花数万 token 探索，只回吐 1000 至 2000 token 结论。一批五张卡若各顶到 4096，总量反而超过单次子代理回吐的十倍量级，把省下来的上下文又吃回去。
- 工具回执正文不入卡，只留 `receipt_ref` 与指纹。这与主流的工具结果清理策略同源：可重新获取的内容不必驻留上下文，只保留调用记录与引用，需要时按引用回取。
- 超长轮次按工具边界分段：每段不超过 16 条回执，或在出现目标切换标记处切段；每段一张卡，由 `segment_index` 与 `segment_count` 标识。单轮最多保留 8 张卡，第 8 张直接吞并剩余全部回执（`evidence_range` 覆盖到末条），`segment_count` 仍报真实逻辑段数，不另造 `merged` 卡。
- `hints` 是检索起点而非结论，允许来自工具参数里的文件路径、符号名、命令首段。当前实现有意留空：轨迹只存参数指纹、不存参数原文，凭空造抓手等于伪造证据；等采集层把参数原文摘要一并落盘后再填充。因此风险表里「靠 hints 降低盲搜」这一层目前不存在，卡片引导只靠 `evidence_range` 与 `top_evidence`。
- 卡片不含任何用户原文，只含指纹与长度，符合 I3。

反思代理拿到卡片后的标准动作序列：先 `builtin.memory_search` 核对既有认知，再对需要的卡片调 `builtin.turn_trace` 取工具原文，必要时用 `notes_*` 或 `history_*` 补历史，最后才决定写不写、写什么。

### 4.3 SalienceVector（不合成单一标量）

五维，全部 basis point（0 至 10000）：

| 维度 | 语义 | 对应现有信号 |
|---|---|---|
| utility | 对未来任务的复用价值 | `future_utility` |
| persistence | 结论跨时间保持成立的概率 | `stability` |
| verification | 运行时执行证据的闭环强度 | `verification_strength` |
| specificity | 是否指向具体对象（文件、命令、符号、字段），而非泛泛而谈 | 新增 |
| novelty | 相对既有认知的新颖度 | `novelty` |

不合成单一标量的理由：单一重要性分数会把「未来有用」「与个人相关」「社会意义」混成一个数，导致记忆的留档理由不可解释，事后无法审计「凭什么记住这条」。分维之后，准入规则可以按候选类型挑维度组合，理由直接可读。

现有 `SemanticSalienceSignals` 的其余三维不废：`identity_relevance`、`correction_strength`、`scope_confidence` 保留为辅助判定维度，分别参与个人类候选的加权与作用域归属校验。本次重构只新增 `specificity`，不推翻既有字段。

### 4.4 三态记忆动作

| 动作 | 触发场景 | 落点 | 可逆 |
|---|---|---|---|
| 新增 Add | 出现可复用新结论 | 新 `memory_id`，治理态为 Weak 或 Strong | 可后续淘汰或归档 |
| 淘汰 Retire | 已召回的旧结论被证明失效或错误 | 旧 id 置 `MemoryState::Tombstone` 并写 `tombstone.reason`（不含原文），同时新建记录并 `supersedes_memory_ids` 指向旧 id | 不可逆，正文不再暴露 |
| 归档 Archive | 旧结论暂不需要但未失效 | 写 `archived_at_ms`，正文保留 | 可 unarchive |

明确不做 UPDATE（合并改写）。理由有三：

1. 合并改写是唯一真正需要版本管理的动作，砍掉之后记忆退化为只追加，语义大幅简化。
2. 零 schema 变更：现有 `memory_supersessions`、`supersedes_memory_ids`、`tombstone`、`archived_at_ms` 已能完整表达三态。
3. 纠正旧认知等价于「新增 + supersede」，版本链天然形成有向无环图，追溯能力不降低。

归档的逆操作是 `MemorySqliteStore::unarchive_records`：只清 `archived_at_ms`，不动正文与状态机；已 tombstone 的记录不会被它复活，归档与撤销两侧都按 `state` 排除淘汰记录。

配额：单轮反思最多新增 3 条，单批最多 5 条；超出按显著度降序截断，截断事实写入回执的 `reason`。


---

## 5. 工具契约

### 5.1 新增 `builtin.turn_trace`

反思代理补齐「当前轮未归档」的唯一入口。已归档的轮次走 `history_*`，两者边界不重叠。

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `intent` | string | 是 | 调用目的，与其他内置工具口径一致 |
| `receipt_ref` | string | 三选一 | 精确回取单条工具回执原文，最省 |
| `trace_id` | string | 三选一 | 取整轮的工具回执切片 |
| `session_id` + `sequence` | string + integer | 三选一 | 按会话与轮序定位。`sequence` 是 `rollout_turns.sequence`（会话内轮序），不是 `replay_chain` 的会话祖先链下标，两者不可互换 |
| `max_chars` | integer | 否 | 单次返回上限，默认 8000，上限 32000 |

寻址规则：`trace_id` 决定轮次，`receipt_ref` 决定该轮内的哪一条，两者可以叠加（此时 `receipt_ref` 退化为轮内过滤）；只给 `receipt_ref` 时必须同时给出 `trace_id` 或 `session_id` 加 `sequence`——脱离轮次的 `receipt:<n>` 不唯一，工具直接拒绝而不猜轮次。三组都不给时按当前会话最新一轮取整轮切片。

返回结构：

```json
{
  "traceId": "trace:7:18",
  "turnId": "turn-18",
  "matchedReceipts": 3,
  "truncated": false,
  "truncatedReceipts": 0,
  "totalBytes": 1280,
  "receipts": [
    {
      "receiptRef": "receipt:3",
      "toolName": "builtin.shell_command",
      "status": "verified",
      "outputRef": { "byteLen": 1280, "sha256": "…" },
      "outputChars": 412,
      "truncated": false,
      "output": "..."
    }
  ]
}
```

边界约束：

- 只读，`permission_policy` 为只读级别，不进入 `GovernedWrite`。
- 跨 owner 或跨 session 的 `receipt_ref` 一律拒绝，不做部分返回。
- `truncated=true` 时给出被截断的回执数（`truncatedReceipts`）与本轮工具输出总字节数（`totalBytes`），让调用方知道还有多少没看到。
- 找不到 `receipt_ref` 时明确报「引用不存在或已被保留窗口回收」，不返回空数组冒充成功。
- 单次调用计入反思线的工具调用预算（见 6.2）。

### 5.2 复用既有工具

| 工具 | 在反思链中的职责 | 契约不变项 |
|---|---|---|
| `builtin.memory_search` | 先核既有认知，避免重复写入与误判冲突 | `query` / `limit` / `since_ms` / `until_ms`，时间窗必须成对给出 |
| `builtin.memory_write` | 唯一写入通道 | `kind` / `summary` / `evidence` / `supersedes_memory_ids` / `intent` 必填 |
| `notes_read_file` / `notes_append_to_file` | 跨窗口保留未完成线索 | 虚拟路径，不支持绝对路径与 `..` |
| `history_list_items` / `history_read_item` / `history_search_contents` | 已归档窗口的精确回溯 | 只对已归档窗口可见 |

三条使用纪律：

1. 先查后写。没有 `memory_search` 命中的核对结论，不得直接写入同主题新记录。
2. `supersedes_memory_ids` 只在「已召回的具体记忆被本轮证据证明失效或错误」时填写；相关但不矛盾的一律不填。
3. 临时线索走便签，长期结论走记忆，两者不互相替代。

---

## 6. 攒批与时序

### 6.1 触发条件

候选池是一个纯函数判定的队列，任一条件命中即触发一次记忆线：

| 条件 | 参数 | 默认值 | 说明 |
|---|---|---|---|
| 条数越线 | `BATCH_MIN_CANDIDATES` | 5 | 攒够 5 张卡片 |
| 累计显著度越线 | `BATCH_MIN_ACCUMULATED_BP` | 40000 | 池内权重达到两张执行底线即成批；一张刚过线的卡撑不起一次批处理 |
| 单卡强档 | `SINGLE_CARD_MIN_BP` | 20000 | 等于执行类准入的三维下限之和（verification 7000 + utility 6500 + specificity 6500）。达标卡片与改造前同频单跑，攒批不会把该当场记住的轮次拖后；低于底线的弱卡才进池，由条数、累计与池龄接管。该值由准入规则反推，不靠拍脑袋，改准入不同步改它会被单测拦下 |
| 显式纠正 | `correction_strength` | 8000 | 用户当场指出错误，立即单跑；池非空时连同池内卡片一起跑，避免与未处理线索冲突 |
| 池年龄越线 | `BATCH_MAX_AGE_MS` | 1 800 000 | 最老卡片满 30 分钟 |
| 会话收口 | `session_closing` | 调用方传入 | 语义上存在，但没有显式 close 事件可挂，生产调用方恒传 false，因此当前不参与判定 |
| 惰性补漏 | `STALE_POOL_MS` | 7 200 000 | 进程启动后的首次判定且池龄超 2 小时才补跑一次；常规轮次不判这一条，避免每次加载都强行跑批 |
| 同簇复现 | `RECURRENCE_MIN_CARDS` | 3 | 同桶内「工作区类别 + 目录 + 主导工具名」这个确定性簇键相同的卡片满 3 张即触发。卡片不含正文，语义相似度无从算起，宁可漏簇也不乱并簇 |

只靠按量触发会漏掉低频场景：一天只跑两三轮深度任务时，5 张卡片永远攒不满。因此量级条件不能是唯一入口，上表八条各自独立，任一命中即跑；其中会话收口依赖尚未接线的显式 close 事件，实际可达七条。

极端稀疏的情况（一个月一次的会话）连 30 分钟窗口与 2 小时补漏都跨不过会话生命周期，靠的是「卡片入池即持久化 + 下次启动补跑」，不依赖进程存活。

按量、按时间之外还有第三个维度：按语义簇的复现密度。只按条数攒批会把五件互不相干的小事凑成一批，反思代理看不出模式，只能逐条判，等于白跑一次；而同一模式在三轮里反复出现，本身就是「值得沉淀」的强信号。这条思路与近年记忆构建的主流结论一致：先把交互放进轻量层用嵌入检索，等语义相似的交互持续复现，再调昂贵的主模型做抽取。公开报告的口径是，这类复现触发能把记忆构建的 token 成本压掉接近一个数量级，同时准确率不降。

池本身按地点分桶，与记忆作用域划分同源：按「目录 + git 分支」分桶，`~/.axon` 单独一个桶。不跨桶合并，避免把两个项目的线索攒到一起。分支名由 `admit_reflection_batch` 读 `<cwd>/.git/HEAD` 得到（只读一个文件，不起子进程）；非 git 目录、detached HEAD、worktree 读不到时按目录兜底，宁可少分一个桶也不猜错分支。

### 6.2 预算与隔离

| 项 | 记忆线 | 人格线 |
|---|---|---|
| 单次预算 | 30 s | 8 s |
| 累计预算 | 未实现：`parent_total_cost_micros` 恒 0，无 5% 门禁 | 不设累计，单轮固定 |
| 工具调用上限 | 未实现：反思子代理沿用通用工具循环，无专用计数器 | 0 次 |
| 超预算行为 | 中止并保留池，记 `FailedNonFatal` | 放弃本轮回写，沿用上一轮状态 |
| 批内输入总量 | 8192 字节 | 不适用 |
| 触发维度 | 卡数、累计显著度、同簇复现、年龄、收口、补漏 | 每轮固定 |

父任务终态先返回，记忆线在后台推进。任何超时或失败都不改写父任务结果，也不丢卡片；卡片丢失只在保留窗口过期时发生，回执按实际结果记 `Persisted` / `NotAttempted` / `SkippedNoChange` 之一，不引入独立的 dropped 概念。

崩溃安全的三条前提：

1. 卡片入池即 append 到本地点桶的池文件（JSONL），不等 flush。进程在任意时刻被杀，已入池的卡片都在盘上。
2. 池文件以 `turn_id` + `trace_id` + `segment_index` 为幂等键。文件损坏、重复写入或补跑重放时按幂等键去重，不会重复入池。
3. 三本台账（`data/runtime/reflection_processed.jsonl`、`reflection_pool_inflight.json`、`reflection_written.jsonl`）分工明确：processed 记「这一轮跑到哪一步」（`started` / `done`，只有 `done` 让重试整条跳过；崩在中途的只剩 `started`，会重跑）；inflight 记「这批已出池、还没跑完」；written 按「批次键 + 候选指纹」记「这条候选已写进库」，挡住半批失败后的重试重写。
4. 出池顺序是「先写 inflight、再回写池」。反过来的话，崩在两步之间会让这批卡片既不在池里也不在 inflight 里，整批静默消失。池文件的 `load → append → rewrite` 全程持跨进程锁（`reflection_pool.lock`），避免两个治理 worker 互相吞卡；锁文件超过 60 秒视为崩溃残留，允许抢占。
5. 池文件不是事实源，只是索引。它损坏时已入池卡片会丢；由于显著度无法从 `rollout_turns` 复原（那里只有证据，没有五维评分），当前不自动重建——重建只能凭零显著度造弱卡，等于往池里灌垃圾并每 30 分钟白烧一次子代理。丢池的代价是「这些轮次不再被反思」，证据本身仍在。

### 6.3 时序

```text
turn 收口
  │
  ├─ A 采集（同步）
  │    TurnTrace + Card 入池
  │    入池即 append；未触发则本轮不跑
  │
  ├─ C 人格（并发）
  │    signals + 有界语义 -> persona_state
  │
  └─ 池判定
       越线 -> B 记忆（异步）
                ├─ 卡片 -> turn_trace / memory_search 取原文
                ├─ 产出候选（动作 + target_id + evidence_refs）
                ├─ 准入（纯函数，见 7.1）
                ├─ 写入（Add / Retire / Archive）
                └─ 写 reflection_receipt 回执
```

回执是可见性的地基。每一次池判定、每一次写入、每一次失败都要留一行，字段沿用现有回执结构：父会话、父 cell、记录时间、hook、`source_trace`、状态、成本、写入决策、原因、摘要指纹、hook 内迭代数。

---

## 7. 准入与人格线

### 7.1 按类型组合维度

不合成标量，但也不能让模型自由心证。准入按候选类型挑维度组合，全部满足才允许写入：

| 候选类型 | 必需维度（全部不小于阈值） | 附加硬条件 |
|---|---|---|
| profile / preference | persistence 7000、utility 7000、scope_confidence 7000 | `evidence_refs` 必须含 `user_message_ref` |
| experience / knowledge / procedure | verification 7000、utility 6500、specificity 6500 | 至少一条 `runtime_verified=true` 且非 Skill 调用的执行证据 |
| style | 不经记忆线 | 由人格系统承接，见 [人格系统](./persona.md) |

弱档只降低「是否值得跑」的门槛，不降低「是否允许写入」的门槛。弱档反思启动后，准入仍按上表强标准判定：达不到就不写，回执记 `skipped_below_admission`。反思产出不使用治理态 Weak 承接，也不写低置信条目。这样库里每一条反思产物都经过同一套证据与维度门禁，强档与弱档的差别只体现在「跑不跑」，不体现在「写多宽」。

`specificity` 是 v4 新增维度，兼容口径按评估输出自带的 `schema_version` 分流。v4 起直接用模型给的值；v3 及更早没有该字段，反序列化落 0，此时用 `verification_strength` 截到 6500 作为折中值。净效果是旧输出在执行类准入上与升版前完全等价：verification 达标时折中值恰好过线，不达标时折中值更小、本来就会被 verification 门槛挡住，既不放松也不收紧。评估输出的接受窗口是 `[3, 4]`，低于 3 或高于 4 的整轮丢弃，避免旧版本被无限期接受，也避免未来版本被当前代码误读。

### 7.2 人格线独立

现状耦合点：人格所需的 `user_affect`、`user_fatigue`、`persona_drift`、`style_candidate` 与记忆所需信号由同一次轻量评估产出，同一次模型调用承担两个目标。记忆线一旦攒批，人格就会被拖成批次级，`persona.md` 里逐轮累加的转移表会失真。

拆分后的边界：

| 维度 | 人格线 | 记忆线 |
|---|---|---|
| 频率 | 每轮 | 攒批 + 显式触发 |
| 模型档位 | `FastDelegate` | 继承父代理主模型 |
| 工具 | 关闭 | 开启，上限 12 次 |
| 输入 | `TurnTrace.signals` + 有界语义 | `ReflectionCard` + 按需 pull 原文 |
| 输出 | `persona_state` | Memory 记录 + 反思回执 |
| 单次预算 | 8 s | 60 s |

人格输入补一层零模型的客观信号 `ExplicitSignals`：

| 字段 | 语义 | 用途 |
|---|---|---|
| `hour_bucket` | 时段分桶（深夜 / 上午 / 下午 / 晚间） | 触发夜间篇幅收敛 |
| `consecutive_turns` | 同一会话连续轮数 | 判断连轴转 |
| `turns_last_30min` | 近 30 分钟轮次密度 | 判断是否在赶工 |
| `input_len_delta` | 相对上一轮的用户输入长度差 | 已采集；当前规则层不据此判定，留给后续注入 |
| `affect_sequence` | 最近 5 轮 `user_affect` 序列 | 情绪走向而非单点判断 |
| `failure_streak` | 连续失败轮数 | 直接驱动 `incident` 模式 |

融合规则：规则层可独立给出降档结论（例如深夜 + 高密度，可直接判定疲劳为 high），不必等语义层；语义层给出的 `user_affect`、`user_fatigue`、`persona_drift` 作为覆盖值参与 `persona.md` 既定转移表。规则层不判读情绪，只输出一档精力与一句可读原因：`ExplicitSignals` 里没有正文，从阈值往外推「用户是什么状态」就是编造。篇幅与语气不另开维度——人格域的 `base_voice` 已经以 `fatigue_level` 为输入做收敛，平行档位只会出现两套口径互相打架。本节只补输入侧，不改转移表、不改体积预算；动到状态机的只有「连败取本地自增与观测值的较大者」这一处对齐，以及评估器缺席时用规则原因顶替 `last_cause`（见 9 节 R4）。

---

## 8. 与现状的差异对照

| 维度 | 现状 | 目标 |
|---|---|---|
| 反思输入 | 有界摘要 JSON，无历史 | 索引卡 + 按需检索工具，原文零截断 |
| 触发 | 每轮二值硬门禁 | 候选池攒批 + 显式纠正立即跑 + 收口 flush |
| 门禁 | 单点否决整轮 | 分维组合准入，强档与弱档两档启动，写入门槛统一 |
| 记忆动作 | 只有新增 | 新增 / 淘汰 / 归档，无 UPDATE |
| 人格 | 与记忆共用一次评估 | 独立评估、每轮、无工具 |
| 界面提示 | 从规划期空集合推导，恒为否 | 由真实写入结果回写 |
| 来源标识 | 按记忆类型映射，无反思维度 | 显式反思来源 + 子代理会话 ID |
| 审计回执 | 唯一调用点不可达，从不落盘 | 每次池判定与写入都落盘 |
| 失败语义 | 与无价值等同处理 | 失败与判否分开计数、分开告警 |

---

## 9. 迁移分期

每期独立可交付、可回滚。前一期未验收通过，不进入下一期。

### R0 技能反思移除（已完成）

实现状态：已落地（2026-11-13）。

- 删除技能进化全链，见 1.6 节的删除范围与依据。运行时代码不再存在第二条「反思产出」通道，`reflection_yields` 从此只反映记忆写入。
- 删除后复核：`SkillDraftProposal`、`SkillEvolution*`、`run_skill_evolution*`、`memory skill-evolution-*` 在 `crates` 与 `scripts` 下零匹配；`cargo check -p axon-cli --all-targets` 通过。
- 回滚：改动集中在文件删除与字段摘除，`git revert` 单个提交即可整体回退；运行态 `~/.axon/skill-evolution/` 目录未被删除，回滚后状态文件仍可读。
- 验收：反思链路的目标测试全部通过，且质量表不再出现「产出 1 轮但实际只有草案」的情况。

### R1 观测与止血（零行为变更）

- 界面提示已改为由真实结果回写：`publish_turn_artifacts` 在「本轮确有写入」时写「新增、淘汰或归档 N 条记忆」，在「卡片已入池待跑」时写「线索已入候选池」，其余不写提示。规划期恒为 false 的 `memory_updated_info_visible` 字段已删除。提示、`turn_trace` 与回执三份产物必须合成同一条 record 只提交一次：`commit_session_to_rollout` 是按 `(session_id, assistant_part_id)` 整条覆盖，分两次提交会让后一次用旧 record 把前一次刚写的字段抹掉。
- 反思回执已在本轮收口处直接落盘：`append_reflection_receipt` 调用 `SelfIterationReceiptStoreRuntime::append_record`，按「有写入 / 已入池待跑 / 跑了无产出」三态分别记 `Persisted` / `NotAttempted` / `SkippedNoChange`；零调用的调度器链路已整体删除，含 `memory_auto_scheduler` 与 `memory_auto_candidate` 两个模块。
- 来源可审计口径：回执 `source_trace` 固定为 `self-iteration:memory_update:<session>:<assistantPartId>`，按此前缀即可筛出全部反思写入。不新增 `MemorySourceOrigin` 变体，因为反思经运行时统一写入、无法随调用携带来源，新增无消费方的枚举只会增加维护面。
- 验收：跑一轮真实会话后，运行态回执文件出现记录；按来源维度能筛出反思产物；界面提示与实际写入一致。
- 回滚：三项都是增量字段与旁路写入，回退到旧行为只需去掉读取侧。

### R2 证据线

实现状态：已完成（2026-11-13）。契约、工具定义、执行层、指纹采集与轨迹落盘全部落地：`TurnTrace` 含 `EvidenceRange.rejected_count` 与指纹注入入口，`builtin.turn_trace` 打通三选一寻址与归档回取，轮次记录新增 `turn_trace` 字段并由 post-run 采集写回。验收测试：runtime 侧 9 个用例（含 `rejected_count` 与指纹注入），CLI 侧 9 个用例（三选一寻址、引用失效报错、max_chars 边界）。

- 新增 `TurnTrace`、`ToolReceiptRef` 采集，落 `rollout_turns` 扩展字段。`ExplicitSignals` 在本期只落类型与默认值；六字段的真实采集已由 R4 接通，见 9 节。
- 新增 `builtin.turn_trace`，打通 `receipt_ref` 到原文的点查。
- 验收：任取一轮历史会话，能凭回执引用取回工具原文；超过 `max_chars` 时 `truncated` 与总量如实上报。
- 回滚：新工具与采集都可独立关闭，关闭后退回摘要输入。

### R3 记忆线改造

实现状态：进行中（2026-11-13 接线轮）。契约层三块已落地：`ReflectionCard` 索引卡（含 4096 字节单卡预算、`evidence_range` 永不裁剪的裁剪顺序、按工具边界切段）；`ReflectionPool` 候选池（八条触发入口、幂等键、地点分桶、批内 8192 字节预算）；`ReflectionCandidate` 准入与三态动作（按类型挑维度、单轮 3 条与单批 5 条配额、Retire 与 Archive 判定）。

接线已打通四段：

1. `specificity` 进评估输出 schema，版本号升到 v4，同时保留 `[3, 4]` 接受窗口与 v3 折算口径（见 7.1）。
2. 卡片由 `ReflectionCard::from_trace` 从真实轨迹投影：按工具边界切段、溢出段并入末卡、`evidence_range` 计数取真实值、超预算按 `hints` 到 `top_evidence` 的顺序收敛。
3. `ReflectionPoolStoreRuntime` 落地池的 JSONL 持久化与重建：幂等键去重、坏行跳过、批后回写剩余队列；反思改为「卡片先入池、再看八条入口是否触发」，未触发则本轮不跑、留待下一次治理。
4. `candidates_from_assessment` 把评估信号经 `to_salience_vector` 桥到 `ReflectionCandidate.signals`，准入代码不再关心评估版本。

阈值已定稿并做过基线核对。2026-11-13 全量质量表（本机 560 条观测）：评估完成 476 次、真正派生反思 88 次、产出记忆 62 次、跑了但零产出 26 次。这 26 次是 29.5% 的纯损耗，也是「单卡强档只认准入底线、低于底线的卡一律进批」的直接依据：单跑一张刚过线的卡，有三成概率白烧一次子代理。据此把 `SINGLE_CARD_MIN_BP` 定在 20000（执行类准入三维下限之和），`BATCH_MIN_ACCUMULATED_BP` 定在 40000（两张底线），两者都由常量关系锁死，改准入不同步改阈值会被单测拦下。

质量表已新增 `reflection_deferred` 列并写入：池内待跑与「评估判否」从此分开统计，不再都记成 `attempts=0`。显式纠正入口也从「池内恒 0」改成从队列表述投影 `correction_bp`，这条入口不再是死代码。

接线状态（2026-11-13 审计后复核 + 补强）：三态动作与配额已进入写入链路（见 R3.5），本节原「仍待接入」一句已过期。当前只剩一条缺口：会话收口强制 flush 未挂到显式关闭事件（本文档唯一允许保留的留白，靠 30 分钟池龄入口兜底）。惰性补漏入口已接通——`trigger_input` 的 `stale_pool_age_ms` 改由池内最老卡片年龄投影，且 Skip 档轮次不再提前返回，命中补漏入口时会强行兜一次底。`same_cluster_cards` 已改由确定性簇键投影，不再是恒 0（原句作废）。

审计后修正的三处实现口径（与本节上文表述不同处，以此为准）：

- 单卡强档判据不是「五维之和 ≥ `SINGLE_CARD_MIN_BP`」，而是「verification / utility / specificity 三维同时达到执行类准入底线」。五维之和与三维门槛之和量纲不同，按和比较会把任何五维均值 4000 的弱卡判成强卡，池被每轮抽干、攒批形同虚设。
- 池条目按地点分桶：`ReflectionPoolEntry.bucket` 由 `pool_bucket_key(cwd, branch)` 生成，判定只装本桶条目，回写时把别的桶原样并回，避免跨项目成批。
- 幂等键是 `turn_id:trace_id:segment_index`：同一轮切多段卡时各段共用 trace_id，只用 turn 加 trace 会把第 2 段起静默判成重复丢弃。

- 任务消息由摘要改为索引卡；候选池与攒批触发；三态动作与配额。
- 弱档只放宽启动门槛，不放宽写入门槛；不写低置信条目。
- 验收：回放历史轮次，反思触发率与产出率相对基线提升，无证据写入为 0，全部产出通过同一套强标准准入。
- 回滚：池可按地点分桶独立关闭，单桶关闭即回到每轮判定。

### R3.5 写入闭环与评估器加固

实现状态：已完成（2026-11-13）。这一段把 R3 的契约真正接进写入链路。

**分工改成硬边界**：反思子代理不再调用 `builtin.memory_write`，只交一份候选报告；准入、配额与三态动作由运行时在收口时执行。这样「写了几条、为什么写」从子代理的自觉变成可复核的运行时行为。

- 候选契约：`ReflectionReportDraft{candidates[], skipped[]}`，每条候选含 kind、summary、evidence、evidenceRefs、targetMemoryId。单次最多 8 条候选，正文与证据各限 600 字，空条目直接丢弃并计数。
- 解析：取正文里最后一个完整 JSON 对象，且必须带 `candidates` 或 `skipped` 字段，避免把一段无关 JSON 当成空报告。
- 落地：候选经 `plan_reflection_actions` 过准入与配额后，用 `AgentMemoryWriteRuntime` 统一写入；`Add` 直写，`Retire` 带 `supersedes` 指向目标记忆；`Archive` 走 `archive_records`，只写 `archived_at_ms`、退出召回但保留正文，属可逆动作，成功计入本轮写入条数。没有目标 ID 的淘汰与没有目标 ID 的归档无处落地，如实记入 `reflection_skipped_candidates`，不假装写成功。（原句「全部归档动作都记入 skipped」已作废，归档能力已落地。）
- 回执：`reflection_notice` 追加未通过准入的候选条数，用户可见「写了 N 条、另有 M 条未通过」。

**评估器加固**：把 `is_valid_for` 拆成 `schema_is_trustworthy`（只判格式与取值域）与证据校验两段，并在输出被判无效时重试一次。评估提示词补一段判定口径，写死「什么算值得记」，避免一律保守判否；判 false 时必须写清理由。

**归档已落地**：`MemorySqliteStore::archive_records` 只写 `archived_at_ms`，记录退出召回但不删内容、不建 supersession 链，属可逆动作；`AgentMemoryWriteRuntime::archive` 把它接到反思收口，`Archive` 动作不再只记账。归档成功计入本轮的写入条数，提示措辞随之改为「新增、淘汰或归档 N 条记忆」，不再只说「新增或更新」。

**同簇复现改成确定性簇键**：卡片不含正文，语义相似度无从算起，因此 `cluster_key` 用「工作区类别 + 目录 + 主导工具名」这个确定性组合。同一地点同一类工具反复出现三次即触发，说明它是该地点的常规工作流。这是可解释的启发式，宁可漏簇也不乱并簇，不再依赖尚未接通的嵌入。

**解析失败可观测**：子代理没按契约输出时写一行警告日志（含正文长度与开头 120 字），不再与「本轮确实没什么可记」在账面上混成一团。

**仍缺**：会话收口 flush 没有显式 close 事件可挂，继续靠 30 分钟池龄入口兜底。

### R4 人格线独立与夹具常态化

实现状态：已完成（2026-11-13）。规则层落在 `persona_rules.rs`：纯函数、零 IO、零模型，只输出一档精力与一句可读原因。篇幅与语气不另开维度——人格域的 `base_voice` 已经以 `fatigue_level` 为输入做收敛，再给一套平行档位只会出现两套口径互相打架，且没有消费方。

- **信号真填**：`collect_explicit_signals` 从 rollout 账本与时钟取字段。`consecutive_turns` 取本轮轮序加一；`turns_last_30min` 按落盘时间在 30 分钟窗口内计数（治理队列异步消费，窗口会整体后移，因此它衡量「这台机器最近多密」而不是「用户几点说的」）；`input_len_delta` 与 `failure_streak` 从最近 24 轮投影，`failure_streak` 只统计本轮之前；`hour_bucket` 由 `hour_bucket_of` 按东八区常量换算（`axon_time` 只有 UTC、不引时区库，偏移做成参数以便换地）；`affect_sequence` 自举——收口时把本轮评估出的 `user_affect` 追加进轨迹，下一轮读回上一轮序列，不新增存储，也不编造 `unknown`。
- **性能**：账本读取用 `json_extract` 只做标量投影，不反序列化整条 `record_json`。单条 record 含完整工具回执与事件流，24 条可达数百 KB，整条解析的成本远超它提供的信息量；投影后只取 prompt 长度、failure 存在性与 affect 序列，全部在 SQLite 侧完成。采集失败写 warning 后按默认值结算，不让「规则层从不生效」变成第二个静默故障。
- **接入方式（三条真实通路）**：`settle_persona_turn` 收 `signals`，规则层只补评估器没给的部分。
  1. `fatigue_level`：评估器缺席时用规则档，模型给了就以模型为准；它进 `resolve_expression` 的 `base_voice`，直接决定表达收敛强度。
  2. `observed_failure_streak`：状态机取「本地自增」与「观测值」的较大者，跨进程重启不再把「连续第 N 次失败」从 1 重数（陈旧观测也拉不回计数）；它进 `resolve_mode` 的 incident 判定。
  3. `rule_cause`：评估器没给情绪判定时，规则层的可读原因成为本轮 `last_cause`，下一轮出现在人格状态段的 cause 里——这条不是给日志看的，是给下一轮模型看的。
- **等价性**：`rule_advice_for` 对「全零信号」直接返回 `None`。全零等于没采集，这道闸保证「开关关闭」与「信号缺席」两条路径逐字段等价，而不是靠概率对齐。
- **回滚**：`PERSONA_RULES_ENABLED_DEFAULT` 置 `false` 即退回旧评估器路径；信号采集本身是旁路只读，不写账本。
- **夹具常态化**：`reflection_replay_tests.rs` 用冻结的一轮（固定时刻、固定回执分布、无正文）进常规测试，断言四条——证据区间覆盖全部回执、20 与 200 条回执的首卡字节数涨幅不超过 10% 且单卡不超过 4096 字节、同一夹具重放逐字段相等、准入判定与时间戳和池状态无关。夹具不读本机库：`data/runtime` 在现场常常是空的，靠真实记录做门禁等于没有门禁。
- 验收：人格结算路径不读池状态，攒批开关只影响反思何时跑，因此两种模式下逐轮一致；夹具在两种模式下都是同一份输入，均通过。

---

## 10. 回放验证

### 10.1 数据源

| 源 | 用途 |
|---|---|
| `rollout_turns`（`session_id` + `sequence` + `record_json`） | 轮次骨架与工具回执指针；每条回执带 `summary` 与 `output_ref`，不含输出原文 |
| `output_ref` 指向的归档文件 | 工具输出的完整原文，按引用定位，受保留窗口约束 |
| `memory_records` / `memory_supersessions` | 既有产出与版本链，用于去重与冲突校验 |
| `memory_runtime_quality` | turn 级计数基线，用于触发率与产出率对比 |
| `self_iteration_receipts.jsonl` | R1 之后的新增回执，用于可见率核对 |

### 10.2 夹具形态

把某一轮的 `record_json` 与它引用的归档文件一并冻结为 fixture，`TurnTrace` 与 `ReflectionCard` 的生成做成纯函数，断言四条：

1. 证据完整性：每条 `ExecutionEvidenceRecord` 至少出现一次对应 `receipt_ref`，不静默丢证据。
2. 卡片有界性：20 条与 200 条回执两个 fixture 各取**首卡**（两者同为 12 条回执），字节数差不超过 10%，且每张卡都小于等于 4096 字节。这里必须比同段位的卡：段数本身会随回执数增加，拿总字节数相比测到的是段数而不是卡片尺寸。这条比单纯断言上限更能防回归，它直接钉住「单卡尺寸不随任务长度线性增长」。
3. 幂等性：同一 fixture 重放两次，动作序列完全一致。
4. 准入一致性：同一 fixture + 同一信号向量，准入结论稳定，不受时间戳与池状态影响。

### 10.3 基线指标

| 指标 | 定义 | 目标 |
|---|---|---|
| 触发率 | 反思尝试 / post-run 轮次 | 相对基线提升，且不靠放宽证据门槛 |
| 产出率 | 写入轮次 / 反思尝试 | 相对基线提升 |
| 无证据写入率 | 无 `runtime_verified` 证据的写入 / 全部写入 | 严格为 0 |
| 用户可见率 | 出现界面提示的写入 / 全部写入 | 接近 100% |
| 审计完整率 | 有对应回执的池判定 / 全部池判定 | 100% |

---

## 11. 风险与取舍

1. **攒批延迟可见性**。最坏 30 分钟才跑一次，用户当下察觉不到「这台机器记住了」。
   取舍：三条出路各自兜一层。单卡够强立即单跑，显式纠正立即单跑，会话收口强制 flush；界面上以「已记下，稍后整理」这类即时提示承接期待，而不是等批次结束才说。
2. **pull 式检索抬高成本**。反思代理每批最多 12 次工具调用，单批成本高于摘要式。
   取舍：卡片带 `hints` 降低盲搜，累计预算封顶在父任务成本的 5%，超预算保留池而非丢卡；宁可不写，不写没证据的。
3. **新增工具扩大攻击面**。`builtin.turn_trace` 能读工具原文，是最敏感的新增面。
   取舍：只读、白名单寻址、跨 owner 拒绝、单次字符上限、按保留窗口回收；不做批量导出，不做跨会话聚合。
4. **三态无 UPDATE 会产生多条渐进记录**。同一事实逐步细化时，库里会出现一串相似条目。
   取舍：靠 supersede 链与摘要相似度去重收敛；对齐 episodic 只追加的范式，代价是库体积略增，收益是每条都可解释、可追溯、可回退。
5. **新增来源枚举属对外契约变更**。HTTP、桌面端与 CLI 都序列化记忆记录。
   取舍：新增枚举值向后兼容，旧客户端忽略未知值；同步更新 [记忆管理系统](./memory-management.md) 与 [内置工具契约](./tool-schema.md)，避免文档与实现分叉。
6. **规则层信号可能被误用**。时段、密度、长度趋势都是启发式，直接硬熔断会误伤。
   取舍：规则层只影响表达策略与反思频率，不改任务执行、不截断内容、不对用户下人格判定；判定理由进告警而非进记忆。
7. **分段可能切错边界**。工具边界不等于语义边界，一个目标可能横跨两段。
   取舍：切错只影响单卡的聚焦度，不影响可回溯性，因为每段 `evidence_range` 覆盖自己的全区间且可从任一段跨段回取；分段策略是可调参数，先按工具边界落地，稳定后再引入语义边界。
8. **反思代理本身就是有偏评估者**。它写下的结论会经记忆跨时传播：即使巩固过程完全正确，带偏的输入仍是传播的充分原因；污染率低到两成就能观察到传播，而且不同偏差类型下巩固的效果可能相反，有的偏差被削弱、有的被放大。
   取舍：反思写入必须带 `evidence_refs` 与反思档位，任何一条都能回溯到具体轮次；反思结论不进事实源，只作候选认知；淘汰与归档提供纠偏通道。不追求一次写对，追求写错了能查出、能撤回。
9. **记忆会反向驱动行为**。长期记忆里存下的偏好会被不当用于后续工具选择，形成工具漂移。
   取舍：`preference` 只收可执行的行为约束，不收随手偏好；`scope_confidence` 与 `persistence` 双阈值把关；表达风格由人格系统承接、不进记忆，从源头切断漂移面。
10. **治理队列重试会重跑整条反思**。子任务失败时该 payload 会按治理队列的重试策略再执行一次，语义评估与反思子代理随之重跑；池的幂等键只挡「卡片重复入池」，挡不住「已经写进库的记忆再写一遍」。
   取舍：用三本台账收敛。`reflection_processed.jsonl` 按 `session:turn` 记 `started` / `done`，只有 `done` 让重试整条跳过；`reflection_written.jsonl` 按「批次键 + 候选指纹」挡住半批写入的重试重写，批次键由本批 `card_id` 排序后取摘要，池顺序变化不影响去重。残留窗口只剩「子代理跑完、`done` 尚未落盘」的毫秒级间隙：此时重试会重跑一次，但同一批候选按指纹跳过，代价是重复一次子代理成本，不是重复写记忆。


---

## 12. 业界对照

本节把设计放到公开实践里校准。只取可公开访问的一手来源（厂商文档、官方仓库、论文页面），不引二手转述；外部结论只作对照，不构成本仓库的事实依据，阈值也不照搬，因为样本分布不同。

| 主题 | 主流做法 | 本文取舍 | 差异原因 |
|---|---|---|---|
| 反思触发 | 显著性累积越线；近年另加语义复现密度 | 八条独立入口，含累计显著度与同簇复现 | 累积阈值是经典默认，单点阈值会一次否决整轮 |
| 记忆动作 | 早期四态（新增、更新、删除、无操作），现行演进为单遍只追加 | 三态：新增、淘汰、归档，不做更新 | 只追加已是主流默认，但纯只追加会让时变事实矛盾累积，必须补淘汰与 supersede |
| 巩固时机 | 会话边界批量，不逐条消息；写操作离开热路径 | 攒批、收口 flush 与惰性补漏 | 抽取调用比存储贵 10 至 100 倍，逐条跑不划算 |
| 证据驻留 | 工具结果可重新获取就清出上下文，只留调用记录 | 卡片只留引用与指纹，正文按引用回取 | 上下文压缩里代价最低的一招 |
| 块尺寸 | 带标签、描述与字符上限的块 | 单卡 4096 字节，批内 8192 字节 | 记忆块以字符上限约束上下文占用，量级参考 5000 字符 |
| 评分维度 | 多数用单一重要性分数 | 五维不合成标量，按类型组合准入 | 单一标量会把未来有用、个人相关、社会意义混成一个数，留档理由不可审计 |
| 巩固策略 | 四维：重要性、合并、衰减、淘汰 | 逐维显式声明，见下表 | 巩固策略四问是评估记忆系统的常用框架，含糊即漂移 |
| 淘汰纪律 | 多依赖模型自行判断过期 | 显式 tombstone 加 supersede | 模型对用户预设的旧状态高度敏感，会顺着旧前提走 |

本文的巩固策略逐维取值：

| 维度 | 取值 |
|---|---|
| 重要性 | 五维分维打分，按候选类型组合阈值，不合成标量 |
| 合并 | 不做合并；纠正旧认知走新增加 supersede，版本链天然有向无环 |
| 衰减 | 沿用现有半衰期模型，标准缺省 30 天，命中即刷新活跃时间 |
| 淘汰 | 失效用 tombstone 并写原因；暂不需要用 `archived_at_ms`；两者都留审计痕迹 |

对照之后有三处判断被反向加固：

1. 不做 UPDATE 得到支持。一线实现已从四态收敛到单遍只追加，说明并行维护四态分支的复杂度收益比不划算。
2. 但纯只追加不够，同样有实证。公开缺陷报告指出，只追加会让现雇主、现城市这类时变事实矛盾累积，新事实不顶掉旧事实。本文用淘汰加 supersede 补上这一环，比纯只追加更完整。
3. 卡片预算不能只定单卡上限。子代理回吐摘要的量级是千级 token，单卡顶格的设计会在一批五张时总量失真，因此改成单卡防御上限加批内总量上限的双层约束。

## 13. 相关文档

- [记忆管理系统](./memory-management.md)：存储模型、检索投影、生命周期与管理接口
- [人格系统](./persona.md)：人格分层、状态机、转移表与体积预算
- [核心概念](./concepts.md)：Interaction、Goal、Execution、Session 与上下文分层
- [内置工具契约](./tool-schema.md)：内置工具名称、字段与权限级别
- [故障诊断与自愈](./troubleshooting.md)：运行态取证与常见异常处置

工具定义要件与其他内置工具完全一致，缺一即视为未完成：

| 要件 | 口径 |
|---|---|
| schema 版本 | `TURN_TRACE_TOOL_SCHEMA_VERSION`；字段增删或语义变化时递增，不复用旧版本号 |
| `intent` | 必填，`ToolSchemaFieldKind::NonBlankString`，描述复用 `TOOL_INTENT_FIELD_DESCRIPTION`；漏填时整条调用判为参数非法，与 `memory_search`、`memory_write` 同源 |
| description | 先一句话说清能力，再逐条写寻址优先级、跨会话拒绝、引用失效报错、`intent` 必填；只写运行态事实，不写内部路径与实现细节 |
| examples | 给两条完整调用示例（单回执寻址、整轮寻址），`intent` 与业务字段同批出现，模型不必猜字段顺序 |
| 权限 | `ToolPermissionPolicy::ReadOnly`，不进 `GovernedWrite` |
| 并发 | `ToolConcurrencyPolicy::ParallelSafe` |
| 输出 | `ToolOutputPolicy::BoundedSummary` |
| 字段范围 | `sequence` 最小 0；`max_chars` 最小 1、最大 32000 |

执行层落点（照此实施，不必重新探路）：

- 名称映射：`tools/orchestrator.rs` 里形如 `"builtin.memory_search" => ToolKind::MemorySearch` 的映射，`turn_trace` 在此加一行。
- 执行分发：同文件 `match request.kind` 分支加 `ToolKind::TurnTrace => execute_turn_trace(tool_call, context)`。
- 分类分支：`run_progress_views.rs` 的显示名与 `run_tool_result_cache.rs` 的缓存分类各需一个分支，漏了编译器会提示。
- 执行体范式：`tools/orchestrator_builtin.rs` 的 `execute_memory_search`。用 `context.config_path` 取本地根、`tool_call.input.as_json()` 取参、`failed_tool_input_receipt` 报参数错误。
- 数据链路：`config_path` 推出运行态根，归档目录在其下；轮次骨架走 `run_rollout_store::load_session_turn(config_path, session_id, assistant_part_id)`，因此 `sequence` 必须先映射到 `assistant_part_id`；工具原文由回执 `output_ref` 指向的归档文件提供。
