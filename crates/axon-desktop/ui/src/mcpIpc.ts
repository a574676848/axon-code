/*
 * MCP 运行态的 IPC 封装。
 *
 * 与 `ipc.ts` 里的 MCP 配置命令（`mcp_list` / `mcp_upsert`）分开：那几条读写
 * `.mcp.json`，这两条读的是 daemon 实际加载成了什么。分开还有一个直接用处：
 * 运行态只服务指标条与 `/mcp`，不必让配置页也把这两条命令拉进依赖。
 */

import { call } from './backend';
import type { McpRuntimeStatus } from './mcpTypes';

/**
 * 读取工作区已加载的 MCP 服务器。
 *
 * 结论来自 daemon 的启动快照，与 turn 用的是同一份；桌面端不自己连 MCP。
 */
export const mcpRuntimeStatus = (cwd: string): Promise<McpRuntimeStatus> =>
  call('mcp_runtime_status', { cwd });

/** 清健康负缓存并重新探测 MCP；一次重载是真的重连一遍。 */
export const mcpRuntimeRefresh = (cwd: string): Promise<McpRuntimeStatus> =>
  call('mcp_runtime_refresh', { cwd });
