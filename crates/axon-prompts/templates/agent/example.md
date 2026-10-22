# Agent Provider 运行时提示词示例

这是一份发送给 Provider 前已经完成变量替换的请求示例。`{{ ... }}` 仅表示动态值的来源；实际 wire 请求中会替换成正文。跨 Workspace 稳定前缀永远位于最前，Workspace、Pane、Skill、记忆召回和本轮运行约束位于历史消息之后，因此切换工作地点不会改变稳定前缀的 hash。

## 角色与来源

| 来源 | Axon 语义 | 变化范围 | OpenAI Responses | OpenAI Chat | Anthropic |
|---|---|---|---|---|---|
| 稳定系统提示词 | `system` | 跨会话稳定 | `instructions` | `system` | 顶层 `system[]` |
| 委托策略 `<delegation-policy>` | `developer` | 跨会话稳定；主 turn 与子代理共用 | `input[].role=developer` | 降级为 `system` | 顶层 `system[]` |
| 工具编排协议 `<tool_orchestration_protocol>` | `developer` | 跨会话稳定；仅主 turn 装配 | `input[].role=developer` | 降级为 `system` | 顶层 `system[]` |
| 记忆窗口协议 `<memory_window_protocol>` | `developer` | 跨会话稳定；主 turn 与子代理共用 | `input[].role=developer` | 降级为 `system` | 顶层 `system[]` |
| `~/.axon/SOUL.md` | `system` | 用户可修改；跨 Workspace 稳定 | `instructions` | `system` | 顶层 `system[]` |
| `~/.axon/AGENTS.md` | `developer` | 用户可修改；跨 Workspace 稳定 | `input[].role=developer` | 降级为 `system` | 顶层 `system[]` |
| 全局 Personal Memory | `developer` | 用户级；跨 Workspace 稳定，内容变化时整体失效 | `input[].role=developer` | 降级为 `system` | 顶层 `system[]` |
| Workspace `AGENTS.md` | `developer` | 随工作地点变化；动态尾部 | `input[].role=developer` | 降级为 `system` | 动态顶层 `system[]` |
| User/Workspace Skill 与 MCP、Pane/CWD/Git、权限、运行约束 | `developer` | 会话能力或工作地点变化时重建；动态尾部 | `input[].role=developer` | 降级为 `system` | 动态顶层 `system[]` |
| Ledger goal/plan、Workspace Memory recall、Context Snapshot | `developer` | 持久任务或本地点动态；动态尾部 | `input[].role=developer` | 降级为 `system` | 动态顶层 `system[]` |
| 用户对话 | `user` | 当前请求与历史 | `input[].role=user` | `messages[].role=user` | `messages[].role=user` |
| Agent 回复 | `assistant` | 历史消息 | `input[].role=assistant` | `messages[].role=assistant` | `messages[].role=assistant` |

`developer` 是 Axon 的统一指令语义，不要求每个 Provider 原生支持该 role。适配器只做协议映射，不把 Workspace 或运行时事实伪装成用户消息。

## Axon 内部消息顺序

```text
<stable system prompt>
<delegation-policy>{{ delegation_policy }}</delegation-policy>
<memory_window_protocol>{{ memory_window_protocol }}</memory_window_protocol>
<tool_orchestration_protocol>{{ tool_orchestration_protocol }}</tool_orchestration_protocol>
<assistant-profile-context>{{ soul_md }}</assistant-profile-context>
<user-agents-context>{{ user_agents_md }}</user-agents-context>
<personal-memory-context>{{ personal_memory }}</personal-memory-context>
{{ conversation history: user / assistant / tool }}
<workspace-context generation="{{ workspace_generation }}">{{ workspace_agents_md }}</workspace-context>
<workspace-transition-context>{{ workspace_transition_receipt }}</workspace-transition-context>
<available-skills-context dedupe="user-wins">
  <global>{{ user_skill_winners }}</global>
  <workspace>{{ workspace_skill_winners_without_global_duplicates }}</workspace>
</available-skills-context>
<available-mcp-context dedupe="user-wins">
  <global>{{ user_mcp_winners }}</global>
  <workspace>{{ workspace_mcp_winners_without_global_duplicates }}</workspace>
</available-mcp-context>
<agent-pane-context>pane={{ pane_id }} cwd={{ cwd }} shell={{ shell }}</agent-pane-context>
<runtime-constraints>{{ runtime_constraints }}</runtime-constraints>
<agent-context-snapshot>{{ ledger_and_memory_context }}</agent-context-snapshot>
{{ current_user_message }}
```

以上顺序中的稳定 System Prompt、Soul、用户级 AGENTS、全局 Personal Memory 和不含地点信息的通用规则构成跨 Workspace 可复用的稳定区域；历史之后的能力与 Workspace 感知内容均属于动态尾部。`global` 表示从 `~/.axon/skills` 或 `~/.axon/.mcp.json` 加载的 User 项，`workspace` 表示从当前 Workspace 加载且未与 User 重名的项；同名项只渲染一次。

Workspace 切换时，运行时递增 `workspace_generation`，先从 session registry 移除旧 Workspace 的 AGENTS、Memory recall、Skill、MCP 与 Pane/CWD 投影，再加载新 Workspace 的对应内容，并重新生成整个动态尾部。新一代请求中不保留旧 Workspace 正文；`workspace_transition_receipt` 只投影切换是否完成，不把旧内容重新带入 Provider。User 级 Skill 与 MCP 不卸载。从 Home 启动且尚未选择地点时，`workspace` 段为空。主代理加载 Soul 与工具策略；由主代理创建的子代理请求既不包含 `assistant-profile-context`，也不装配 `<tool_orchestration_protocol>`，但装配 `<memory_window_protocol>`，其稳定前缀逐字节等于主代理稳定前缀去掉 `<tool_orchestration_protocol>` 后的真前缀，因此两类请求仍能共享 provider 前缀缓存。

## OpenAI Responses

```jsonc
{
  "model": "{{ model_id }}",
  "instructions": "{{ stable_system_prompt }}\n\n<assistant-profile-context>\n{{ soul_md }}\n</assistant-profile-context>",
  "input": [
    {"type":"message","role":"developer","content":[{"type":"input_text","text":"<user-agents-context>\n{{ user_agents_md }}\n</user-agents-context>"}]},
    {"type":"message","role":"developer","content":[{"type":"input_text","text":"<personal-memory-context>\n{{ personal_memory }}\n</personal-memory-context>"}]},
    {"type":"message","role":"user","content":[{"type":"input_text","text":"{{ previous_user_message }}"}]},
    {"type":"message","role":"assistant","content":[{"type":"output_text","text":"{{ previous_assistant_message }}"}]},
    {"type":"message","role":"developer","content":[{"type":"input_text","text":"<workspace-context generation=\"{{ workspace_generation }}\">\n{{ workspace_agents_md }}\n</workspace-context>\n<workspace-transition-context>\n{{ workspace_transition_receipt }}\n</workspace-transition-context>"}]},
    {"type":"message","role":"developer","content":[{"type":"input_text","text":"<available-skills-context dedupe=\"user-wins\">\n<global>\n{{ user_skill_winners }}\n</global>\n<workspace>\n{{ workspace_skill_winners_without_global_duplicates }}\n</workspace>\n</available-skills-context>\n<available-mcp-context dedupe=\"user-wins\">\n<global>\n{{ user_mcp_winners }}\n</global>\n<workspace>\n{{ workspace_mcp_winners_without_global_duplicates }}\n</workspace>\n</available-mcp-context>"}]},
    {"type":"message","role":"developer","content":[{"type":"input_text","text":"<agent-pane-context>\n{{ pane_cwd_context }}\n</agent-pane-context>"}]},
    {"type":"message","role":"developer","content":[{"type":"input_text","text":"{{ runtime_constraints }}\n{{ ledger_and_memory_context }}"}]},
    {"type":"message","role":"user","content":[{"type":"input_text","text":"{{ current_user_message }}"}]}
  ],
  "tools": [{{ openai_responses_tool_schemas }}],
  "tool_choice": "auto",
  "stream": true
}
```

## OpenAI Chat Completions

```jsonc
{
  "model": "{{ model_id }}",
  "messages": [
    {"role":"system","content":"{{ stable_system_prompt }}"},
    {"role":"system","content":"<assistant-profile-context>\n{{ soul_md }}\n</assistant-profile-context>"},
    {"role":"system","content":"<user-agents-context>\n{{ user_agents_md }}\n</user-agents-context>"},
    {"role":"system","content":"<personal-memory-context>\n{{ personal_memory }}\n</personal-memory-context>"},
    {"role":"user","content":"{{ previous_user_message }}"},
    {"role":"assistant","content":"{{ previous_assistant_message }}"},
    {"role":"system","content":"<workspace-context generation=\"{{ workspace_generation }}\">\n{{ workspace_agents_md }}\n</workspace-context>\n<workspace-transition-context>\n{{ workspace_transition_receipt }}\n</workspace-transition-context>"},
    {"role":"system","content":"<available-skills-context dedupe=\"user-wins\">\n<global>\n{{ user_skill_winners }}\n</global>\n<workspace>\n{{ workspace_skill_winners_without_global_duplicates }}\n</workspace>\n</available-skills-context>\n<available-mcp-context dedupe=\"user-wins\">\n<global>\n{{ user_mcp_winners }}\n</global>\n<workspace>\n{{ workspace_mcp_winners_without_global_duplicates }}\n</workspace>\n</available-mcp-context>"},
    {"role":"system","content":"<agent-pane-context>\n{{ pane_cwd_context }}\n</agent-pane-context>"},
    {"role":"system","content":"{{ runtime_constraints }}\n{{ ledger_and_memory_context }}"},
    {"role":"user","content":"{{ current_user_message }}"}
  ],
  "tools": [{{ openai_chat_tool_schemas }}],
  "tool_choice": "auto",
  "stream": true
}
```

## Anthropic Messages

Anthropic 没有 message-level `system` 或 `developer` role。Axon 将稳定消息先转换为顶层 `system[]`，再追加不属于 stable prefix 的动态 system blocks；由于协议不允许在 `system[]` 与 `messages[]` 之间交错，动态 blocks 在 wire 上位于历史消息之前，但始终排在稳定 system blocks 之后。历史对话仍只使用 `user` 和 `assistant`。

```jsonc
{
  "model": "{{ model_id }}",
  "system": [
    {"type":"text","text":"{{ stable_system_prompt }}"},
    {"type":"text","text":"<assistant-profile-context>\n{{ soul_md }}\n</assistant-profile-context>"},
    {"type":"text","text":"<user-agents-context>\n{{ user_agents_md }}\n</user-agents-context>"},
    {"type":"text","text":"<personal-memory-context>\n{{ personal_memory }}\n</personal-memory-context>"},
    {"type":"text","text":"<workspace-context generation=\"{{ workspace_generation }}\">\n{{ workspace_agents_md }}\n</workspace-context>\n<workspace-transition-context>\n{{ workspace_transition_receipt }}\n</workspace-transition-context>"},
    {"type":"text","text":"<available-skills-context dedupe=\"user-wins\">\n<global>\n{{ user_skill_winners }}\n</global>\n<workspace>\n{{ workspace_skill_winners_without_global_duplicates }}\n</workspace>\n</available-skills-context>\n<available-mcp-context dedupe=\"user-wins\">\n<global>\n{{ user_mcp_winners }}\n</global>\n<workspace>\n{{ workspace_mcp_winners_without_global_duplicates }}\n</workspace>\n</available-mcp-context>"},
    {"type":"text","text":"<agent-pane-context>\n{{ pane_cwd_context }}\n</agent-pane-context>"},
    {"type":"text","text":"{{ runtime_constraints }}\n{{ ledger_and_memory_context }}"}
  ],
  "messages": [
    {"role":"user","content":[{"type":"text","text":"{{ previous_user_message }}"}]},
    {"role":"assistant","content":[{"type":"text","text":"{{ previous_assistant_message }}"}]},
    {"role":"user","content":[{"type":"text","text":"{{ current_user_message }}"}]}
  ],
  "tools": [{{ anthropic_tool_schemas }}],
  "stream": true,
  "max_tokens": {{ max_output_tokens }}
}
```

实际请求还会根据 Provider 能力加入 prompt cache、reasoning、structured output、附件和 tool-call 字段；这些字段不改变稳定区在前、动态区在后的角色边界。
