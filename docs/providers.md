# Provider 与模型

本章说明 Provider 配置管理、连通性探测与模型目录命令。参数和协议枚举以 `axon providers --help`、`axon providers add --help` 的当前输出为准。

## Provider 管理

```bash
axon providers [add|remove|test]
```

| 子命令 | 说明 |
|--------|------|
| 省略子命令 | 列出所有 Provider |
| `add` | 添加 Provider |
| `remove <NAME>` | 移除 Provider |
| `test <NAME>` | 使用配置同目录的 `.env` 凭据请求 Provider `/models` |

```bash
axon providers
```

### 添加 Provider

```bash
axon providers add --name openai --protocol openai \
  --base-url https://api.openai.com/v1 --api-key-env OPENAI_API_KEY
```

`--protocol` 支持 `openai`、`openai-chatcompletions`、`anthropic`、`dashscope`、`volcengine-ark-multimodal`。底层传输默认为 HTTP；明确支持 WebSocket 的 Provider 可传 `--wire-transport websocket` 与可选的 `--websocket-url`。

```bash
# 添加 OpenRouter
axon providers add --name openrouter --protocol openai \
  --base-url https://openrouter.ai/api/v1 --api-key-env OPENROUTER_API_KEY

# 添加 DashScope
axon providers add --name dashscope --protocol dashscope \
  --base-url https://dashscope.aliyuncs.com/api/v1 \
  --api-key-env DASHSCOPE_API_KEY

# 添加支持 WebSocket 的 Provider
axon providers add --name realtime --protocol openai \
  --base-url https://api.example.com/v1 --api-key-env EXAMPLE_KEY \
  --wire-transport websocket --websocket-url wss://api.example.com/v1/realtime
```

### 探测 Provider

```bash
axon providers test openai
```

探测命令根据 Provider 的 `api_key` 或 `api_key_env` 解析凭据（自动加载对应环境或 `~/.axon/.env`），并请求 `<base_url>/models`。

### 移除 Provider

```bash
axon providers remove openai
```

## 模型目录

```bash
axon models [--provider <NAME>] [--format <FORMAT>]
```

| 选项 | 说明 |
|------|------|
| `--provider <NAME>` | 仅列出指定 Provider 的模型 |
| `--format <FORMAT>` | 输出 `text` 或 `json` |

```bash
axon models
axon models --provider openai --format json
```

`axon run --model <MODEL>` 接受模型路由键或模型 ID。同一路由键下配置多个 Provider 时，运行时仍按 `priority`、`weight`、会话亲和与健康状态执行渠道选择和故障转移。

`enabled = false` 的 Provider 一条路由都不参与：它的模型不出现在 `axon models`、TUI `/model` 候选与桌面端的选择器里，已有会话记录指向这些模型时回退 `agent.default_model`。配置改写后这些清单会自己跟上，`axon models` 每次读取即重算，TUI 在配置文件变化后自动复读，桌面端在设置页写成功时复读。

同一 `session_id` 的渠道亲和在进程内跨线程共享，并在每次命中时按 `channel_balancer.affinity_ttl_ms` 滑动续期，因此长任务、自动压缩和子代理执行不会因线程切换或间隔较长而无故漂移到其他 Provider。只有亲和渠道过期、被健康熔断，或当前请求发生可转移的瞬时失败时，才会按候选链切换渠道；切换成功后会重新绑定该会话的亲和渠道。

渠道故障转移以单个渠道的 3 次尝试为界：首次请求加 2 次重试仍失败后，重试预算交给渠道链的下一个渠道，不再继续在同渠道等待。切换后会在同一个 run 内重跑 Provider turn loop。每次渠道尝试都从上一尝试之后的 turn 序号接着编号，assistant/reasoning/lifecycle 的 part 身份因此在整轮内唯一：新尝试的正文不会拼进上一尝试的流式单元，失败诊断记录也按尝试分段而不是无分隔拼接。

失败属于连接或上游链路问题（连接超时、没等到响应头、上游流无法继续转发）且本轮还没产出任何模型输出时，渠道切换不会重放已落地的副作用：Runtime 保存失败 turn 的起始历史与本轮已完成的工具事实，下一渠道从同一位置续跑，因此即使本轮已经写过文件也允许换渠道。

渠道性能遥测默认只保存在进程内。开启 `channel_balancer.telemetry_persistence_enabled` 后，样本会按 UTC 日期追加到 `~/.axon/data/channel-metrics/samples-YYYY-MM-DD.jsonl` 分片；可使用 `axon debug channel-metrics` 按时间、模型和 Provider 查询。内置 Cron 会按会话保留窗口清理过期分片。

```bash
axon debug channel-metrics [--since-hours <HOURS>] [--model <MODEL>] [--provider <PROVIDER>] [--limit <COUNT>]
```

debug 查询会先按时间跳过不可能命中的日期分片，再按最新分片优先逐行应用模型和 Provider 条件，并最多接纳 `--limit` 个渠道进行聚合；结果在这个查询预算内按动态评分保留最优渠道（默认 50），避免把全部历史遥测合并到内存和输出。设置 `--since-hours 0` 可取消时间条件；无匹配时不会回退读取进程内存指标。

## Prompt Cache 遥测

每次 Provider 请求都会按 UTC 日期向 `~/.axon/data/prompt-cache-metrics/samples-YYYY-MM-DD.000000.jsonl` 追加一行缓存形状样本（单分片上限 4 MiB，超出滚到 `.000001` 依此类推）：Provider、模型、协议、尝试序号、缓存断裂原因（首轮、纯追加、system 或 tools 或请求选项变化、历史被改写等）、公共前缀条数与实测命中 token。命中率掉下来时，用它可以判断究竟是换了渠道、工具表变了，还是历史被改写，而不是只看得到「这一轮很贵」。

登记按日志等级分流：`first_request`、`history_appended`、`exact_replay` 属于正常路径，只写 DEBUG；其余原因意味着上一轮已付费的前缀被作废，直接写 WARN，默认日志级别即可看见。会话中途从别的渠道切过来同样等于从零付前缀，样本会带 `channelSwitched: true` 并一并升到 WARN。

样本体量有三道闸：哈希只留 16 位指纹；单进程每自然日封顶 4096 条，超出后只留日志不再落盘；进程内形状记忆按会话分区并限长 32 条，先进先出淘汰。写盘失败与环境无关时只留一条告警，不影响主流程。

## Memory Embedding

`[memory.embedding].model` 必须引用 `supports_embedding = true` 的已配置模型。DashScope 多模态向量模型使用原生 `dashscope` 协议和 `https://dashscope.aliyuncs.com/api/v1`，Axon 会请求 Multimodal-Embedding API，并从官方 `output.embeddings[].embedding` 响应中读取向量。

```toml
[memory.embedding]
enabled = true
model = "qwen3-vl-embedding"

[[models]]
name = "qwen3-vl-embedding"
id = "qwen3-vl-embedding"
provider = "dashscope-multimodal"
supports_embedding = true
embedding_dimension = 1536

[[providers]]
name = "dashscope-multimodal"
protocol = "dashscope"
base_url = "https://dashscope.aliyuncs.com/api/v1"
api_key_env = "DASHSCOPE_API_KEY"
```

`embedding_dimension` 必须使用模型官方支持的维度，并与 Memory 向量索引维度一致。OpenAI-compatible embedding 响应继续使用 `data[].embedding`；火山方舟多模态响应使用 `data.embedding`。

## 思考强度

`agent.reasoning_effort`、`axon run --reasoning-effort` 与 TUI `/think` 使用同一组配置值。输入边界只接受 Provider 官方值并集；公共协商层会在发起网络请求前静默省略当前协议不支持的合法档位。`--reasoning-effort` 与 `/think` 都只覆盖本轮之后的会话请求，不改写 `agent.reasoning_effort`：桌面端与 TUI 的显式强度随提交下发，进程退出即回到配置默认值。复杂推理由主 Agent 使用当前选中模型直接完成，不存在独立的推理子代理模型槽位。

流式投影不重新定义 Provider Markdown 规则。TUI 与桌面端的流式预览和最终回复都先进入共享 `normalize_markdown_source` 语义层，再执行分段或消息编码；因此表格分隔线、全角空格、NBSP 和 fenced code 的处理保持一致。流式预览只承担低延迟展示，最终正文以 canonical assistant 终态为准。

部分 OpenAI-compatible Provider 会把工具调用降级为逐行的 `<function_calls>` 文本块。实时进度投影与最终响应聚合共用同一文本工具调用解析器：只在 fenced code 之外识别完整、独占行的调用块，使用 XML 与 JSON 解析器转换为真实工具调用，并从 TUI 与桌面端可见正文中移除原始标签；代码示例、无效块和未闭合块仍按普通正文处理，流结束时会刷新缓冲内容，避免尾部文本丢失。

| Provider 协议 | API 字段 | 支持值 |
|---------------|----------|--------|
| OpenAI Responses | `reasoning.effort` | `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max` |
| OpenAI Chat Completions | `reasoning_effort` | `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max` |
| Anthropic Messages | `output_config.effort` + `thinking.type=adaptive` + `thinking.display=summarized` | `low`、`medium`、`high`、`xhigh`、`max` |

具体模型可能只支持协议集合的子集。模型明确返回 effort 能力错误时，Axon 会在同一渠道按协议支持值由高到低静默重试，并按 Provider、协议与模型缓存当前进程内成功档位；最低档仍不支持时省略 effort 字段。鉴权、限流、上下文溢出或其他 400 错误不会触发该降级。

### 思考内容来源

- OpenAI Responses：可见思考来自流式 `response.reasoning_text.delta` 或 `response.reasoning_summary_text.delta`。Axon 不会从最终 `response.output_item.done.item.summary` 回填正文；如果上游只在最终事件中提供摘要而没有发送 delta，TUI 不显示空的思考块。
- OpenAI Chat Completions：优先读取流式 `choices[].delta.reasoning`，并兼容 `choices[].delta.reasoning_content`。同一个 delta 同时包含两个字段时只读取 `reasoning`，避免重复展示。
- Anthropic Messages：启用 reasoning 时显式请求 `thinking.display=summarized`，并将流式 `content_block_delta` 中的 `thinking_delta.thinking` 映射为可见思考。

### 思考回传兼容

部分 OpenAI-compatible 网关（例如以 DeepSeek 思考模式承载的网关）要求历史 assistant 消息逐条回传 `reasoning_content`，缺失时整条请求返回 `400 invalid_request_error`：`The reasoning_content in the thinking mode must be passed back to the API.` 该兼容要求按模型声明：

```toml
[[models]]
name = "deepseek-v4.1-flash"
id = "deepseek-v4.1-flash-1017"
provider = "thinking-gateway"
context_window = 1000000
requires_reasoning_content_on_assistant_messages = true
```

开启后，Axon 在 OpenAI Chat Completions 请求体中为每条 assistant 消息回传推理正文：会话内已保存的真实推理正文原样回放，缺失时补空串（上游只校验字段存在性）。该开关只影响 Chat Completions 协议；未声明该兼容要求的模型保持原样，不发送空推理字段。

### 长会话缓存

普通 Coding turn 按稳定前缀、会话稳定上下文、历史、当前轮注入上下文、当前用户输入的固定顺序装配。**当前轮注入上下文（Workspace 与 AGENTS.md、Skills 索引、环境与上下文快照、明文约束等）随该 turn 一并落库**，后续轮次按原位置原字节回放，因此同一会话内 Provider 可见消息只追加、不改写：每一轮都能命中上一轮的全部前缀，只有本轮新内容需要重新计算。历史、注入上下文与工具回执一律不得在下一轮重新推导或改写。

Provider 可见工具集合在同一 run 内保持不变。运行时从最终三协议请求体生成不含明文的 `ProviderVisibleCacheShape`，用于区分历史追加、system/tools 变化和消息前缀重写，并提供 `cacheBreakReason`、`commonPrefixMessages` 等诊断读数，以维持长会话高缓存命中率并最小化 token 成本。

## 相关文档

- [CLI 参考](./cli.md)
- [配置指南](./configuration.md)
- [故障排查](./troubleshooting.md)
- [开源贡献指南](../CONTRIBUTING.md)
