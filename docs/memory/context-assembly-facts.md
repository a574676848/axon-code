# 上下文装配事实核对（M0-2）

> 基线：`master @ 58ca008ec`，分支 `feat/memory-enhance`。
> 方法：逐项读代码，每条结论带 `文件:行号`；无法确认的写「未确认 + 已试路径」。
> 本文是 M7 缓存分区的输入，不重复 README 的设计叙述，只写仓库实际现状。

## 1. `context/` 下是否存在 `sections.rs`，它定义了什么

存在：`crates/axon-agent-runtime/src/core/context/agent_context/sections.rs`（250 行）。

- 它把上下文快照的**单个 section**（`AgentContextSectionPayload`）映射为
  `kind / label / source_ref / content` 四件事：
  `sections.rs:49`（kind）、`:64`（label）、`:79`（source_ref）、`:108`（content）。
- 承载的 section 共 10 类：Cwd、Git、ShellHistory、CommandEvent、CommandActivity、
  AgentApplyHistory、AgentChatHistory、MemoryRecall、LedgerContext、RepoStructure（`:50-61`）。
- 它把 section 归并到预算分类 `ContextSectionKind`：`:148-162`
  （Cwd/Git/RepoStructure → CurrentDiff；五类历史 → History；MemoryRecall → Memory；LedgerContext → Ledger）。
- token 估算采用 4 字节 ≈ 1 token 的粗估：`:10` `TOKEN_BYTES_PER_UNIT = 4`，实现见 `:209-215`。
- `content()` 输出统一经 `redact_agent_context_text` 脱敏：`:144`。
- 可压缩性白名单（预算不足时可摘要）见 `:32-42`。

## 2. 人格四要素分别在哪一层读取与拼接

装配顺序（由 `ContextAssemblerRuntime::assemble_input` 串起）：
`crates/axon-agent-runtime/src/core/context/context_assembler.rs:110-146`。

| 要素 | 读取/构造层 | 落位 | 证据 |
|---|---|---|---|
| Soul（`.axon/SOUL.md`） | session-stable context 分类器 | 稳定层 system 角色 | `crates/axon-agent-runtime/src/agent/agent_session_stable_context.rs:23-25`（`<assistant-profile-context>` marker）、`:35`（`Soul` 枚举）、`:60`/`:132`（分类命中）、`:139`（映射到 `SYSTEM_ROLE`） |
| 用户级 `AGENTS.md` | 同一分类器 | 稳定层，跨 workspace 保持 | `agent_session_stable_context.rs:11`（`# AGENTS.md instructions` marker）、`:156`（只允许 Soul / 用户级 AGENTS / 全局 Personal Memory 进稳定层） |
| Workspace `AGENTS.md` / instructions | workspace instruction 通道，作为 fragment | 动态上下文 | `agent_session_stable_context.rs:11` marker 复用；进入 `ContextAssemblyResult` 的 `stable_context_messages` / `dynamic_context_messages`（`context_assembler.rs:36-44`） |
| `persona_state` / `learned_style` | CLI 运行时从 `persona.sqlite3` 读出后包成 `<persona-state-context>` | 动态消息（历史之后） | `crates/axon-persona/src/infrastructure/sqlite_persona_store.rs:1`（库职责）、`:24`（`PERSONA_SCHEMA_IDENTITY`）、`:28`（`persona_state` 表）；`crates/axon-cli/src/runtime/persona_context.rs:29`/`:31`（marker）；`crates/axon-cli/src/run_commands/run_request_payload.rs:156`（`persona_state_message: persona_context.dynamic`）；`run_model_payload.rs:121`（条件注入） |

拼成的结构：`AgentModelStablePrefix`（system/developer 稳定前缀）+ `stable_context_messages`
（顺序固定、跨 workspace 保持）+ 会话历史 + `dynamic_context_messages`（workspace/环境/persona_state）
+ 本轮用户输入；见 `context_assembler.rs:33-45`、`:110-146`。

> 未确认：`SOUL.md` 磁盘路径的字面量在 Rust 代码内 grep 不到（已试 `crates/**/*.rs` 搜 `SOUL.md`、`soul`）。
> 现状是运行层只认 `<assistant-profile-context>` marker 与 `Soul` 分类，文件读取发生在上游 CLI 启动路径。

## 3. `context_budget.rs` 的常量与分配策略

- `DEFAULT_SUMMARY_TOKEN_ESTIMATE = 128`：`crates/axon-agent-runtime/src/core/context/context_budget.rs:7`。
- 预算请求/计划/回执结构：`ContextSectionInput`（`:36`）、`ContextBudgetRequest`（`:55`）、
  `ContextBudgetPlan`（`:66`）、`ContextSectionReceipt`（`:90`）、`ContextSectionBudget`（`:111`）。
- 规划入口：`ContextBudgetRuntime::plan`（`:214`）。
- **裁剪优先级（数值越小越保）**：Policy 0、Selection 1、CurrentDiff 2、ToolOutput 3、Ledger 4、
  Memory 5、CommandOutput 6、History 7、Attachment 8（`:383-391`）。
  因此超限时裁剪顺序是 Attachment → History → CommandOutput → Memory → Ledger → …
- 不可裁剪集合：`Policy | Selection`（`:398`）。
- 每段上限不写死在常量里，而是按 `ContextSectionInput.token_estimate` 与
  `summary_token_estimate`（`sections.rs:31`，取 64 与估算值的较小者）动态分配；超限时以摘要估算替换。

## 4. system prompt 里是否已含时间戳 / 随机 id / 每轮变化的计数

- **稳定前缀不含时间**：session-stable context 的准入白名单只允许 Soul、用户级 AGENTS、全局 Personal Memory
  （`agent_session_stable_context.rs:156`），没有时间字段。
- **时间字段在动态环境消息里**：
  `crates/axon-agent-runtime/src/core/context/environment_context.rs:20`（`utc_now_ms`）、
  `:22`（`local_time`）、`:57`/`:88`（`render` 输出 `utc_now_ms:` / `local_time:`），
  经 `context_assembler.rs:110` 的 `dynamic_context_messages.push(environment_context.to_message())` 落到历史之后的动态区。
- 结论（M7 输入）：**P2 至少有两个天然变化源**——`utc_now_ms` 与 `local_time`；P0/P1 当前不含秒级时间与随机 id。
  未发现每轮递增的计数器进入稳定层（已试 `SystemTime::now` 全仓 grep，命中的都是 nonce/重试/压缩等非提示词路径：
  `agent_graph_file_store.rs:337`、`agent_graph_store.rs:244`/`:432`、`insert_then_compress.rs:644`、`provider_retry.rs:130`）。

## 5. 不同通道（cli / desktop / voice / http）是否分支上下文或人格

- 通道标识存在：`ClientChannel`（`crates/axon-agent-runtime/src/core/run/agent_turn_runtime/` 下的 turn 输入），
  在 `agent_turn_runtime_tests.rs:17` 等测试里以 `ClientChannel::Cli` 出现。
- **上下文装配层未按通道分支**：`crates/axon-agent-runtime/src/core/context/` 全目录 grep
  `ClientChannel::{Cli,Desktop,Voice,Http}` 零命中；`ContextAssemblerRuntime` 是唯一装配入口（`context_assembler.rs:105`），
  所有通道走同一条 section → budget → payload 链路。
- 人格投影也不按通道分支：`persona_context` 在 CLI 运行时统一构造（`crates/axon-cli/src/runtime/persona_context.rs`），
  调用方只决定是否传 `persona_state_message`（`run_model_payload.rs:121`）。
- 本期结论：M7 的分区只需改一处装配入口即可对全部通道生效；voice 由独立 crate 承载，不在本期范围。

## 附：对后续 Issue 的影响

1. **M0-3 埋点**应挂在 `ContextAssemblerRuntime::assemble_input`（`context_assembler.rs:105-146`），
   分段取 `snapshot`（section 收集）、`inject_memory_recall`、`build_fragments`、`build_with_fragments` 四段。
2. **M2-2 / M3-4 P2 注入**应落在 `dynamic_context_messages` 一侧（历史之后），
   而不是 session-stable context，否则会破坏 `agent_session_stable_context.rs:156` 的缓存准入白名单。
3. **M7 分区**的第一刀应把 `stable_prefix` + `stable_context_messages` 归为 P0/P1，
   `environment_context` / persona_state / 检索记忆 / 线索承诺归为 P2；`sections.rs:148-162` 的预算分类可直接复用为分区依据。
