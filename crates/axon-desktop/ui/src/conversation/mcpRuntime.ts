/*
 * MCP 运行态：指标条的读数与输入坞的 `/mcp` 读的是同一份结论。
 *
 * 结论来自 daemon 的启动快照（与 turn 用的是同一份），因此这里不轮询：进入工作区读
 * 一次，用户显式敲 `/mcp` 时再重载一次。一次重载是真的重连一遍 MCP 服务器，定时跑
 * 会把这件事变成后台常态。
 *
 * 翻译规则（几台正常、确实异常、回执怎么写）在 mcpReading 里，两份消费方共用。
 */

import React from 'react';

import { mcpRuntimeRefresh, mcpRuntimeStatus } from '../mcpIpc';
import type { McpRuntimeStatus } from '../types';
import { mcpCommandReceipt, type McpCommandReceipt } from './mcpReading';

/** 失败原因归一成一句中文。 */
function failureMessage(failure: unknown): string {
  return failure instanceof Error ? failure.message : String(failure);
}

export interface McpRuntime {
  /** 最近一次读数；还没读到时为空。 */
  status: McpRuntimeStatus | null;
  /** 读取失败的原因（网关不可达、daemon 没起来）。 */
  error: string | null;
  /** 正在重载。 */
  reloading: boolean;
  /** 重载一次并返回回执。 */
  reload: () => Promise<McpCommandReceipt>;
}

/**
 * 跟着工作区取一次 MCP 运行态。
 *
 * `cwd` 为空（还没选工作区）时不发请求：界面此刻没有「当前工程」可言，
 * 读到的数不属于任何人。
 */
export function useMcpRuntime(cwd: string | null): McpRuntime {
  const [status, setStatus] = React.useState<McpRuntimeStatus | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [reloading, setReloading] = React.useState(false);

  React.useEffect(() => {
    if (cwd === null || cwd.length === 0) {
      setStatus(null);
      setError(null);
      return undefined;
    }
    let cancelled = false;
    mcpRuntimeStatus(cwd)
      .then((next) => {
        if (cancelled) return;
        setStatus(next);
        setError(null);
      })
      .catch((failure: unknown) => {
        if (cancelled) return;
        setStatus(null);
        setError(failureMessage(failure));
      });
    return () => {
      cancelled = true;
    };
  }, [cwd]);

  const reload = React.useCallback(async (): Promise<McpCommandReceipt> => {
    if (cwd === null || cwd.length === 0) {
      return { ok: false, message: '还没有选中工作区，无法重载 MCP' };
    }
    setReloading(true);
    try {
      const next = await mcpRuntimeRefresh(cwd);
      setStatus(next);
      setError(null);
      return mcpCommandReceipt(next);
    } catch (failure: unknown) {
      const message = failureMessage(failure);
      setError(message);
      return { ok: false, message: `重载 MCP 失败：${message}` };
    } finally {
      setReloading(false);
    }
  }, [cwd]);

  return { status, error, reloading, reload };
}
