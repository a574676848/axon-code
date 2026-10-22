# MCP 集成

本章介绍如何通过 Model Context Protocol（MCP）扩展 Axon 的能力。

## 什么是 MCP

Model Context Protocol 是一个开放协议，允许 AI 应用通过标准化接口访问外部工具和数据源。Axon 提供 MCP 客户端，用于发现、握手并调用外部 MCP server。

### 核心概念

| 概念 | 说明 | 示例 |
|------|------|------|
| MCP 服务器 | 提供工具和资源的独立进程 | 文件系统服务器、数据库服务器 |
| 工具（Tools） | AI 可以调用的函数 | `read_file`、`query_database` |
| 资源（Resources） | AI 可以访问的数据源 | 文档、API 响应 |
| 提示词（Prompts） | 预定义的提示词模板 | 代码审查模板 |

## Axon 的 MCP 支持

### MCP 客户端

通过 `axon-mcp-runtime` 在 turn-time 发现、握手并调用外部 MCP server，支持 `stdio` 与 `streamable_http` 的工具流；runtime 同时提供 `resources/list`、`resources/templates/list`、`resources/read`、`prompts/list`、`prompts/get` 与 `completion/complete` 访问 API。`completion/complete` 完整保留 server 返回的 values 与 `hasMore` 分页信号，client 不再进行固定条数二次裁剪；需要更多结果时由调用方按 MCP 分页协议继续请求。

每个 session 的完整 `ToolRegistry` 是工具定义与授权范围的唯一事实源。Provider 直接接收注册表中的全部工具 schema（含 MCP），模型在已下发的 `mcp__<server>__<tool>` 名称中直接选型并调用完整名称；不存在 capability proxy、激活步骤或能力发现工具。执行层使用同一实例校验和调用真实工具。CLI 的 `--tool` 只在 session registry 派生时应用一次，并同时约束 Provider `tools` 数组与执行层。

MCP 工具注册时同时生成本机权限与并发策略。权限映射不引入人工审批门：工具一旦注册就与 builtin 工具同等可直接调用。只有工具完整声明 `readOnlyHint=true`、`destructiveHint=false`、`openWorldHint=false` 时记为 `ReadOnly`，其余情况（含注解缺失或声明风险）一律记为 `GovernedWrite`；两者在调用级均映射为 `Allowed`。并发策略由同一组注解推导：完整安全注解为 `ParallelSafe`，其余为 `Exclusive`。并发策略不发送给 Provider；MCP 工具仍受 turn-time schema 校验、执行期契约漂移 fail-closed、输出脱敏与熔断治理约束。

## 故障隔离与超时

MCP 失败按阶段治理，粒度与后果不同：

- **发现期**（配置加载、协议握手、`tools/list`、空工具列表、schema 注册）：失败写 **server 级**负缓存，该 server 在窗口内不注册任何工具，也就不会出现在模型可见工具表中。server 级负缓存有效 **30 秒**。
- **调用期**（`tools/call` 的连接、超时或 transport 异常）：失败只写 **(server, tool) 级**冷却，窗口 **10 秒**；该工具在窗口内快速失败，同 server 的其他工具、工具注册表与模型可见工具表都不受影响。调用期失败**永不**写 server 级熔断，也不重建工具注册表。

参数校验失败、server 返回的业务拒绝（JSON-RPC `error`）与本地配置错误（例如 `toolTimeoutMs` 非法）都不写任何负缓存：前两者交回模型修正，后者 fast-fail 并要求先修配置。运行取消（用户中断）是运行期控制事实而不是 server 健康信号，同样不写负缓存；stdio 会终止对应的本机 MCP 子进程。发现期摘除只影响后续的注册表构建，不追溯中断同 server 已在执行的调用。

调用期降级回执的 `error_type` 为 `mcp_unavailable`，metadata 同时给出 `degraded=true`、`degradationClass=optional`、`circuitOpen`（发现期熔断在调用期被命中）与 `toolCooldown`（命中工具冷却），摘要按作用域给出可执行的替代建议；配置错误与业务拒绝是普通失败回执（`error_type=execution_failed`），不标记降级。

恢复路径：工具冷却随窗口过期，该工具调用成功即立即清除；server 级熔断在该 server 发现成功或任一工具调用成功时清除，也可用 TUI `/mcp` 立即重探 —— 它会清空全部负缓存、作废并在后台强制重建该 Workspace 的快照（绕过负缓存与 schema 缓存现场重探），下一轮 turn 即使用新的工具表。Daemon 的启动快照缓存 TTL（5 分钟）到期时，只要上一次快照存在探测失败的 server 就强制重建并重跑探测；反过来说，只发生调用期失败的 server 不会因此被摘除，需要立刻反映真实故障时用 `/mcp`。

Axon 完整保留 MCP `tools/list` 返回的 tool description 与 schema，不设置客户端私有的描述长度上限，也不通过截断规避注册失败。配置解析、协议握手、`tools/list`、空工具列表与 schema 注册失败都会形成独立的 server load report；负缓存保存同一条已脱敏原因。TUI `/mcp` 直接展示对应 server 的实际失败原因，恢复探测会绕过负缓存并用本次结果更新状态。

每个 server 可在 `.mcp.json` 使用 `toolTimeoutMs`（或 `tool_timeout_ms`）为 `tools/call` 设置显式超时毫秒数，值必须大于 `0`；未设置时默认超时为 `300` 秒，非法值（`0`）在调用前 fast-fail 且不写任何负缓存。显式值不受 30 秒上限限制。stdio 握手与 `tools/call` 使用独立预算；运行取消时 stdio 会终止对应的本机 MCP 子进程。Streamable HTTP 的 `tools/call` 使用同一默认或显式请求超时，并继续投影 server 的 `notifications/cancelled` 事件；初始化、schema 发现与资源访问仍使用各自的有限预算。

MCP 真正执行时使用调度当轮的 `execution_tools` 快照校验参数，不使用执行阶段重新发现得到的新 Schema。执行阶段的重新发现仅用于解析 transport 路由和检测契约漂移，不会替代当轮 Schema；若新发现的 Schema、权限或并发策略与当轮快照不一致，本轮调用 fail-closed，要求下一轮基于新契约重新发起。

stdio 工具执行必须在同一 server 进程内依次完成 `initialize`、`notifications/initialized`、支持 `nextCursor` 的 `tools/list`，再发送 `tools/call`。不得为执行阶段另启进程后跳过握手直接调用。server 返回的标准 `result.content` 文本会进入模型可见摘要和 `outputPreview`，`structuredContent` 只作为结构化投影；只有真实回执非空，才可判定工具调用完成。

工具回执会将前 16 个 `result.content` 块投影为脱敏 `contentBlocks`：文本保留有界预览，图片、音频、资源和资源链接只保留类型及必要的 URI、名称或 MIME 类型。TUI 会在 `outputPreview` 中显示非文本结果的类型与数量；不会接收或显示 `data`、`blob`、base64 等二进制载荷。

MCP Tools、Resources、Prompts 与 Completion 在 Axon 中都是外部 server 能力投影。Domain 层不感知 MCP 协议，runtime 负责协议握手、会话 header、安全投影与结果边界。MCP tool 可发现即可直接调用，不存在人工审批分支；安全边界由 schema 校验、契约漂移 fail-closed、输出脱敏与熔断治理承担。

## 快速开始

### 配置来源与优先级

Axon 只读取两个 MCP 配置文件：用户级 `~/.axon/.mcp.json` 与当前 Workspace 的 `<workspace>/.mcp.json`。两者都使用顶层 `mcpServers` 对象。User 优先于 Workspace；User 已提供同名或规范化后同 ID 的 server 时，Workspace 项不会重复加载。

切换 Workspace 后，旧 Workspace 的 MCP server 从当前运行时目录卸载，再从新 Workspace 的 `.mcp.json` 加载非重复项。用户级 server 保持不变。

### 添加 MCP server

```bash
# stdio
axon mcp add filesystem -- npx -y @modelcontextprotocol/server-filesystem /path/to/workspace

# Streamable HTTP
axon mcp add remote-mcp --url https://mcp.example.com/mcp

# 已配置 server 列表
axon mcp list
```

### 查看 server 安全配置摘要

```bash
axon mcp get filesystem
```

CLI 通过 `serverId` 与 `source` 输出安全摘要，不输出启动命令、header 或 token 值。

### OAuth 登录（Streamable HTTP）

Axon 的 Streamable HTTP 客户端按 MCP `2026-07-28` 授权规范实现，覆盖元数据发现、客户端注册、
授权码换取 token 与凭据刷新全流程。

```bash
# 生成授权 URL（不启动本地回调，适合脚本或远程终端）
axon mcp login remote-mcp

# 拉起系统浏览器、等待本地回调，成功后保存 token
axon mcp login remote-mcp --browser

# 显式指定 scope、预注册 client 或 resource indicator
axon mcp login remote-mcp --scope connector.access --client-id <client-id> --resource <resource-uri>

# 清除本地 token 与待完成会话
axon mcp logout remote-mcp
```

TUI 内提供等价入口：`/mcp login <server>` 与 `/mcp logout <server>`。

**授权发现**。客户端按规范顺序解析 Protected Resource Metadata：优先使用 401/403 应答中
`WWW-Authenticate` challenge 的 `resource_metadata` 与 `scope`，其次回退到 endpoint 子路径的
`/.well-known/oauth-protected-resource/<path>`，最后才回退到站点根的
`/.well-known/oauth-protected-resource`。Authorization Server Metadata 只接受真实发现端点返回的
内容，不合成 `/authorize`、`/token` 等路径。

**issuer 校验**。PRM 声明的授权服务器标识符必须与 AS metadata 的 `issuer` 一致（仅容忍根路径
末尾斜杠差异）。授权服务器声明 `authorization_response_iss_parameter_supported` 时，回调必须携带
匹配的 RFC 9207 `iss`，否则该回调被拒绝。

**客户端注册**。优先级为显式 `--client-id`（HTTPS URL 且授权服务器声明支持时记为 Client ID
Metadata Document）→ 动态客户端注册（RFC 7591，仅在授权服务器暴露 `registration_endpoint` 时作为
兼容回退）→ 报错并提示用户提供 `client_id`。动态注册结果按「授权服务器 + redirect URI」缓存在
`~/.axon/mcp-oauth/clients/`，同一授权服务器只注册一次；缓存读取失败按未注册处理，写盘失败不影响
本次登录。

**scope 选择**。优先级为命令行 `--scope` → challenge 声明的 scope → AS metadata 的
`scopes_supported`。

**回调地址**。`--browser` 与 TUI 登录默认绑定 `127.0.0.1:47821`，端口被占用时回退到系统分配的
临时端口。固定端口让 redirect URI 保持稳定，从而复用已注册的 client；回调校验 `state` 与 RFC 9207
`iss`，等待超时为 300 秒。

**401/403 step-up**。server 返回 401，或返回 403 且 `WWW-Authenticate` 声明
`error="insufficient_scope"` 时，Axon 将其投影为「MCP 需要 OAuth 授权」语义，而不是笼统的状态码
异常：CLI 输出与 TUI `/mcp` 列表都会同时给出 `axon mcp login <server>` /
`/mcp login <server>` 引导。其他 4xx/5xx 仍保持通用失败原因。

**启动期探测与 TUI 授权**。进入 Axon 时，首屏 catalog 加载完成后复用同一份后台加载结果检查需要
授权的 server：命中时只提示一次并给出 `/mcp login <server>`，不额外发起网络请求，也不会自动弹出
浏览器。`/mcp login` 在后台线程完成浏览器授权与本地回调，不阻塞 TUI 渲染与输入；成功后自动强制
刷新 `/mcp` 列表，并请求 Daemon 侧重探（清除健康负缓存 + 作废、重建该 Workspace 的启动快照）。

**凭据注入**。tools/list 装配、tools/call 执行与 prompts/resources 调用统一按规范化 server ID 读取
`~/.axon/mcp-oauth/` 中的 access token，过期时先用 refresh token 续期再发起请求；只有显式配置
`bearerTokenEnvVar` 的 server 保持环境变量优先，因此授权结果在重启后依然生效，不需要重新登录。

## 常用 MCP 服务器

| 场景 | 推荐 server |
|------|-------------|
| 文件系统 | `@modelcontextprotocol/server-filesystem` |
| 数据库 | `@modelcontextprotocol/server-postgres` |
| 浏览器自动化 | `@playwright/mcp` |
| Git | `@modelcontextprotocol/server-git` |
| 远程 HTTP | 任意 Streamable HTTP MCP 端点 |

## 自定义 MCP 服务器

任何实现 MCP 协议的进程都可被 Axon 接入。Python 与 Node.js 示例可参考 [Model Context Protocol 官方文档](https://modelcontextprotocol.io)。

### Python

```python
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("my-server")

@mcp.tool()
def hello(name: str) -> str:
    return f"Hello, {name}!"

if __name__ == "__main__":
    mcp.run()
```

### Node.js

```typescript
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

const server = new McpServer({ name: "my-server", version: "1.0.0" });
server.tool("hello", { name: "string" }, async ({ name }) => ({
  content: [{ type: "text", text: `Hello, ${name}!` }],
}));
await server.connect(new StdioServerTransport());
```

添加到 Axon：

```bash
axon mcp add my-server -- python /path/to/server.py
axon mcp add my-server -- node /path/to/server.js
```

## 调试 MCP 服务器

```bash
# 使用 mcp inspector 调试 stdio server
npx @modelcontextprotocol/inspector node /path/to/server.js

# 通过 Axon 验证运行环境
axon doctor --check mcp --verbose
```

## 安全与配置约束

- MCP 工具没有信任开关：注册后即可直接调用，不存在 `trusted` 或写入审批映射。需要收紧某个 server 的可用范围时，用 `--tool` 约束 session registry 的工具集合。
- `bearerTokenEnvVar` 只保存环境变量名，运行时读取变量值并生成 `Authorization: Bearer <token>`；配置、CLI JSON 输出和资源投影都不会输出 token 原文。
- server 名会规范化为小写下划线形式的稳定 `server_id`；User 配置优先于 Workspace 配置，同名或规范化后同 ID 的 Workspace 项不重复加载。
- MCP 配置契约保持收敛：stdio 仅接受 `type`、`command`、`args`、`cwd` 等字段；已移除的 `trusted` 不再被消费。

## 相关文档

- [配置指南](./configuration.md#mcp-服务器配置)
- [开源贡献指南](./CONTRIBUTING.md)
- [CLI 参考](./cli.md#扩展与工具)
- [模型接入与配置](./providers.md)
