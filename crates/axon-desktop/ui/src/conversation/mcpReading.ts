/*
 * MCP 运行态的纯读数：把 daemon 的载荷翻译成指标条那一段与 `/mcp` 的回执。
 *
 * 只做翻译，不取数、不碰 React：指标条与输入坞各自取数，翻译规则只有这一份，
 * 因此「几台正常」「几台异常」在两处的口径永远一致。
 */

import type { McpRuntimeServer, McpRuntimeStatus } from '../types';

/** 指标条要的几件事：一共几台、几台正常、有没有读不到结论的错误。 */
export interface McpHeaderReading {
  totalCount: number;
  loadedCount: number;
  failedCount: number;
  /** 读不到结论时的原因；正常时为空。 */
  error: string | null;
}

/** 一次重载的回执：一句结论加上 `/mcp` 要展开的服务器清单。 */
export interface McpCommandReceipt {
  ok: boolean;
  message: string;
  /** 服务器清单；指令失败或没有结论时缺省。 */
  servers?: McpRuntimeServer[];
}

/**
 * 指标条读数。
 *
 * 没有 MCP 且没有错误时返回 null：指标条据此不画这一段，与 LSP 段「大于 0 才出现」
 * 同一口径。读不到结论（网关不可达、daemon 没起来）反而要画出来，那是需要人知道的
 * 事实，不能悄悄消失。
 */
export function mcpHeaderReading(
  status: McpRuntimeStatus | null,
  error: string | null = null,
): McpHeaderReading | null {
  if (error !== null) {
    return {
      totalCount: status?.totalCount ?? 0,
      loadedCount: status?.loadedCount ?? 0,
      failedCount: status?.failedCount ?? 0,
      error,
    };
  }
  if (status === null || status.totalCount === 0) return null;
  return {
    totalCount: status.totalCount,
    loadedCount: status.loadedCount,
    failedCount: status.failedCount,
    error: status.error ?? null,
  };
}

/** 指标条那一段的文字：异常时先把「几台异常」说清楚。 */
export function mcpSegmentText(reading: McpHeaderReading): string {
  if (reading.error !== null && reading.totalCount === 0) return 'MCP 异常';
  if (reading.failedCount > 0) {
    return `MCP ${reading.loadedCount} · ${reading.failedCount} 异常`;
  }
  return `MCP ${reading.loadedCount}`;
}

/** 悬停时的全文：读数被截断时它是唯一入口，因此把台数与原因都写全。 */
export function mcpSegmentTitle(reading: McpHeaderReading): string {
  if (reading.error !== null) {
    return reading.totalCount === 0
      ? `读不到 MCP 运行态：${reading.error}`
      : `共 ${reading.totalCount} 台 MCP，${reading.error}`;
  }
  if (reading.failedCount > 0) {
    return `共 ${reading.totalCount} 台 MCP，${reading.failedCount} 台加载异常`;
  }
  return `共 ${reading.totalCount} 台 MCP，全部加载正常`;
}

/**
 * 重载回执：说清这一次刷新之后是几台正常、几台异常。
 *
 * 没有服务器也要如实说「没有配置」，不能只回一句「已刷新」让人误以为有东西被加载了。
 */
export function mcpCommandReceipt(status: McpRuntimeStatus): McpCommandReceipt {
  if (status.error !== undefined && status.error !== null) {
    return { ok: false, message: `重载 MCP 失败：${status.error}` };
  }
  const servers = status.servers;
  if (status.totalCount === 0) {
    return {
      ok: true,
      message: '已重载 MCP：这个工作区没有配置 MCP 服务器',
      servers,
    };
  }
  if (status.failedCount > 0) {
    return {
      ok: false,
      message: `已重载 MCP：${status.loadedCount} 台正常，${status.failedCount} 台异常`,
      servers,
    };
  }
  return {
    ok: true,
    message: `已重载 MCP：${status.loadedCount} 台全部正常`,
    servers,
  };
}
