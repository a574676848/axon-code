# 上下文消息契约

本文定义 `AgentModelMessage` 在上下文装配、换窗保留与 Provider 投影中的标记语义，避免把「是否注入」「是否缓存」「谁在说话」混为一谈。

## 1. 三个字段各管什么

一条消息上同时存在三个彼此独立的维度：

| 字段 | 类型 | 回答的问题 | 由谁决定 |
|---|---|---|---|
| `source` | `AgentModelMessageSource` | 内容来自真实会话，还是运行时注入 | 构造点 |
| `kind` | `AgentModelMessageKind` | 是不是特殊类型的内部消息 | 构造点 |
| `role` | `String` | 交给 Provider 时扮演什么协议角色 | 构造器入参或 Fragment 元数据 |

`content` 才是正文。三者都不能单独用来判断「这是不是用户说的话」。

## 2. 取值

### 2.1 `source`（`agent_model_message.rs`）

| 值 | 语义 |
|---|---|
| `Context` | Axon 注入的内容：稳定前缀片段与动态注入都是它 |
| `Conversation` | 用户、assistant 或工具产生的会话消息（默认值） |

`source` 只回答「谁注入的」，不回答「稳不稳定」：稳定前缀与每轮重建的动态上下文同为 `Context`，靠 `kind` 区分。

### 2.2 `kind`

| 值 | 语义 | 是否进缓存 |
|---|---|---|
| `Regular` | 普通模型消息（默认值） | 视 `cache_excluded` |
| `RuntimeNote(AgentRuntimeNoteKind)` | Axon 注入的低权威运行时提示 | 否 |
| `InjectedContext` | 本轮重建的动态上下文：环境、本机资源、话题线索、异常状态、人格动态、子代理提示 | 否 |
| `ConversationCheckpoint` | 历史压缩交接摘要 | 否 |

### 2.3 `retention_class`

| 值 | 语义 |
|---|---|
| `P0Contract` | 用户目标、显式约束与验收标准 |
| `P1State` | 当前状态与可继续执行的结构化事实 |
| `P2Evidence` | 工具输出、命令结果与可由引用取回的证据 |
| `P3Derivable` | 可由其他事实推导的重复或临时内容 |

`retention_class` 只描述保留优先级，不参与来源判定。

## 3. 构造器矩阵

| 构造器 | `source` | `kind` | `cache_excluded` | 典型用途 |
|---|---|---|---|---|
| `text(role, content)` | `Conversation` | `Regular` | `false` | 最终用户请求、assistant 回答、工具消息 |
| `context(role, content)` | `Context` | `Regular` | `false` | 跨轮稳定的前缀片段 |
| `injected_context(role, content)` | `Context` | `InjectedContext` | `true` | 环境信息、本机资源、话题线索、异常状态、人格动态、子代理提示、工具结果图片提示 |
| `cache_excluded(role, content)` | `Conversation` | `Regular` | `true` | 遗留路径；新代码应改用 `injected_context` |
| `runtime_note(kind, content)` | `Conversation` | `RuntimeNote(kind)` | `true` | 预算提醒、换窗提示、工具循环告警 |
| `conversation_checkpoint(summary)` | `Conversation` | `ConversationCheckpoint` | `true` | 换窗交接摘要 |

注意：`context()` 只改 `source` 与 `retention_class`，**不会**把 `kind` 改成特殊值。Fragment 管线按片段元数据分叉：`Ephemeral` 或 `Unbudgeted` 片段投影为 `Context + InjectedContext + cache_excluded`，稳定可缓存片段保持 `Context + Regular`——否则 provider 规范化会把非 `Regular` 一律强制改成不入缓存。

## 4. 判定规则

### 4.1 真实用户请求

`AgentModelMessage::is_real_user_turn`（`agent_model_message.rs:370`）同时要求：

```text
role   == "user"
source == Conversation
kind   == Regular
```

三个条件缺一不可：`role == "user"` 无法排除注入内容；`source == Conversation` 无法区分用户与 assistant；`kind == Regular` 才能排除交接摘要与运行时提示。

### 4.2 跨 Workspace 稳定上下文

`is_cross_workspace_stable_message`（`agent_session_stable_context.rs:52`）只接受：

1. `source == Context`、`cache_excluded == false`、`retention_class == P1State`；
2. 命中 `assistant-profile-context`、`user-agents-context`、`personal-memory-context` 三者之一；
3. 或 role 为 `system` / `developer`，且不含动态 Workspace 标记与运行时提示。

`validate_session_stable_context_messages`（同文件 `:146`）对稳定上下文消息做同一判定，不允许 Workspace、Skill、Pane、快照混入。

### 4.3 换窗保留边界

`active_turn_tail`（`insert_then_compress_history.rs:52`）从最后一条真实用户请求向前扫描，**只**纳入连续满足以下条件的消息：

- `kind == InjectedContext`；
- 且不属于跨 Workspace 稳定上下文。

遇到任何其他消息（assistant 回复、工具结果、稳定前缀、交接摘要）立即停止，不得越过它继续向前捡内容。右边界固定停在当前用户请求，其后已归档的 assistant 回复不再搬进新窗口。

这条边界用 `kind` 而不是 `source`：动态注入与稳定前缀现在同为 `Context`，若继续用 `source != Context` 判定，整段旧对话都会被一路扫进新窗口。

换窗后各类内容的归属（每类只有一个归属，不重复、不遗漏）：

| 内容 | 换窗后行为 |
|---|---|
| 稳定前缀（Soul、全局规则、个人记忆） | 由 `is_cross_workspace_stable_message` 重新装配为 `stable_messages` |
| 紧邻当前用户请求的动态注入 | 随 turn 带入（`kind == InjectedContext`） |
| 更早 turn 的动态注入 | 丢弃，下一轮按需重建 |
| 交接摘要 | 新写入一条 `ConversationCheckpoint` |
| 当前用户请求 | 原样带入 |
| 更早的对话与工具输出 | 只留在归档，按需用 `history_*` 回读 |

### 4.4 Provider 角色转换

`provider_role`（`agent_model_tool_history.rs:282`）在会话已经开始后，把 `system` / `developer` 降级为 `user`，但仅当 `source != Context` 时生效。`Context` 来源的消息保持 `developer`——这也覆盖 `InjectedContext`，所以注入内容不会被降级成用户发言。

`normalize_provider_message`（同文件 `:194`）另会把非 `Regular` 的 `kind` 强制标记为 `cache_excluded`，并清理空工具元数据。

### 4.5 缓存与保留等级

`PromptFragmentPipeline::into_message`（`prompt_fragment_pipeline.rs:61`）按 Fragment 元数据决定：

- `lifecycle == Ephemeral` 或 `budget == Unbudgeted` 时 `cache_excluded = true` 且 `kind = InjectedContext`；否则保持 `kind = Regular`；
- `stability == Stable` 时 `retention_class = P1State`，否则 `P2Evidence`；同时满足 Ephemeral 与 Unbudgeted 时为 `P3Derivable`。

### 4.6 换窗交接摘要

摘要的作者是当前模型，运行时不生成语义内容、不按栏目截断。提交通道与工作便签是两条独立通道：

**提交通道**：模型需要精确控制摘要语义时，把摘要写入私有便签的保留路径 `handoff/summary.md`（`SESSION_NOTES_SUBMITTED_SUMMARY_PATH`），文件首行必须是 `P0:`（`SESSION_NOTES_SUBMITTED_SUMMARY_HEADER`）。首行不是 `P0:` 时该文件按普通便签处理，不参与原样采用。

**工作便签**：`checkpoint.md` 等其余便签保持自由格式，模型可继续用任意 markdown 结构记录进展，不必为换窗改写结构。

**正文结构**：提交摘要固定六段，顺序不可交换、不可重复：

```text
P0:
TASK:
CONSTRAINTS:
VERIFIED:
OPEN:
TOOLS:
```

`TOOLS` 段每行写 `- ID: <provider-call-id>`，列出尚未回填结果的 tool call；没有时必须写 `- none`。结构与活动调用 ID 由 `CanonicalCheckpoint::parse`（`canonical_checkpoint.rs:40`）校验，缺段、空段、顺序错误、ID 缺失都会被拒绝。

**预算**：总量 `4096` tokens（`SESSION_NOTES_CHECKPOINT_MAX_TOKENS`，常量，无配置项）与 `32 KiB` 字节硬上限（`SESSION_NOTES_CHECKPOINT_MAX_BYTES`）；token 由 `estimated_text_tokens` 按 UTF-8 字节数除以 4 向上取整估算。取值以压住换窗固定成本为先：这段摘要每次都要顶到新窗口最前面，4 096 tokens 已能容纳 4 KiB 便签内联（≈1024 tokens）加六段标题与其余段落。超限一律 fail-closed 并返回「请压缩便签后重写摘要」，不截断、不降级采用部分内容。

**兼容回退**：模型未提交（没有 `handoff/summary.md`，或首行不是 `P0:`）时，`SessionNotes::try_render_canonical_summary`（`session_notes.rs:113`）按目标锚点与全部便签投影出同结构摘要。投影对单条便签分两档处理（`NOTES_INLINE_MAX_BYTES = 4 KiB`）：不超过阈值的便签内联正文，超过的只写「路径 + 字节数 + 行数 + 回读指引」，由新窗口用 `builtin.notes_read_file` 按行范围取回。短便签整段可读、长便签只留引用，两条路都不挤爆换窗预算，也不需要模型为换窗压缩或改写。投影与提交两条路径都不做行级截断；替换历史前 `validate_replacement`（`insert_then_compress_completion.rs:171`）再校验一次同一预算，并追加「新窗口 `after_tokens < trigger`」与自动换窗的收敛要求。

## 5. 请求装配顺序

常规请求的 `messages` 顺序固定为：

1. 内置稳定前缀 `stable_prefix`：通用系统提示、委派策略、上下文管理规则、工具使用规则、主代理工具编排规则；
2. 会话稳定上下文 `stable_context_messages`：人格定义、用户级全局规则、个人记忆快照；
3. 历史 `history_messages`：既往用户请求、助手回答、工具调用与结果、已安装的交接摘要、运行时提示；
4. 本轮动态上下文 `dynamic_context_messages`：项目规则、技能目录、人格状态、本机资源、话题线索、异常状态、环境信息；
5. 本轮 Fragment 管线输出：提示上下文、运行约束、上下文快照、Pane 投影、源码符号、已选技能、能力路由；
6. 当前真实用户请求。

工具定义 `model_tools` 独立于 `messages`，不占用消息位次。

## 6. 语义边界

- `cache_excluded` 只表示缓存策略，**不表示** `source`。动态注入消息可以同时是 `Conversation + Regular + cache_excluded`。
- `role == "user"` 不构成用户发言证据：交接摘要与运行时提示的内部 role 同样是 `user`。
- 交接摘要只能靠 `kind == ConversationCheckpoint` 识别，不能靠 `source`。
- 换窗保留判定以 `source` 为准，因此未把 `source` 设为 `Context` 的动态注入消息不会被带入新窗口。

## 7. 相关代码

- `crates/axon-agent-runtime/src/agent/agent_model_message.rs`：枚举、构造器、真实用户请求判定
- `crates/axon-agent-runtime/src/agent/agent_model_payload.rs`：消息装配顺序、稳定前缀编译
- `crates/axon-agent-runtime/src/agent/agent_model_tool_history.rs`：角色转换与元数据规范化
- `crates/axon-agent-runtime/src/agent/agent_session_stable_context.rs`：稳定上下文白名单
- `crates/axon-agent-runtime/src/core/context/insert_then_compress_history.rs`：换窗保留边界
- `crates/axon-agent-runtime/src/context/prompt_fragment_pipeline.rs`：Fragment 到消息的投影
- `crates/axon-agent-runtime/src/core/context/environment_context.rs`：环境信息投影
- `crates/axon-agent-runtime/src/core/context/session_notes.rs`：交接摘要提交契约与总预算校验
- `crates/axon-agent-runtime/src/core/context/session_notes_store.rs`：便签读取与投影（无截断）
- `crates/axon-agent-runtime/src/core/context/canonical_checkpoint.rs`：六段结构与活动 tool call 门禁
