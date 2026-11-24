/*
 * MCP 运行态与配置的载荷类型。
 *
 * 与 `types.ts` 里的 MCP 配置类型（`McpServerItem` / `McpServerInput`）分开：
 * 那两份描述 `.mcp.json` 里写了什么，这里描述 daemon 真的加载成了什么，
 * 两者来源不同、刷新时机也不同。
 */

/** 一台 MCP 服务器的运行态读数；与 daemon 载荷同形。 */
export interface McpRuntimeServer {
  serverId: string;
  /** 该服务器注册的可用工具数。 */
  toolCount: number;
  /** 注册的可用工具名；未加载起来时为空。 */
  tools: string[];
  /** 是否没加载起来：零工具即视为异常。 */
  failed: boolean;
  /** 失败原因（已脱敏）；成功时为缺省。 */
  failure?: string | null;
  /** 快照记录的 schema 处置（hit / stale / miss / refreshed / empty）。 */
  disposition: string;
}

/** MCP 运行态；加载结论来自 daemon 的启动快照，与 turn 用的是同一份。 */
export interface McpRuntimeStatus {
  cwd: string;
  servers: McpRuntimeServer[];
  totalCount: number;
  loadedCount: number;
  failedCount: number;
  /** 读不到结论时的原因；正常时为缺省。 */
  error?: string;
  checkedAtMs: number;
}
