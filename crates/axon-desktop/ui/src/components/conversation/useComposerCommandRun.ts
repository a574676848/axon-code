/*
 * 输入坞指令的执行态。
 *
 * 内置指令是网络动作（`/mcp` 要重连一遍 MCP，通常几秒），因此按「开始、进行中、回执」
 * 三段呈现：开始时立刻给出文案与转圈，进行中每 200ms 更新一次耗时，结束后换成一行
 * 回执并摊开清单。没有这一段，按下回车之后输入框立刻清空，人无法判断到底发出去了没有。
 *
 * 同一条指令重复触发会被忽略：重载是一次真实探测，连点两次只会让第二次等在队列里。
 */

import React from 'react';

import type { McpCommandReceipt } from '../../conversation/mcpReading';
import type { McpRuntimeServer } from '../../mcpTypes';

/** 指令到执行态文案；新增指令时在这里补一句。 */
const COMMAND_LABELS: Record<string, string> = {
  mcp: '正在重载 MCP 服务器…',
};

/** 未知指令的兜底文案。 */
export function commandLabel(name: string): string {
  return COMMAND_LABELS[name] ?? `正在执行 /${name}…`;
}

/** 耗时刷新间隔：够看出它在动，又不至于每帧重渲染。 */
const TICK_MS = 200;

interface RunningCommand {
  name: string;
  startedAtMs: number;
}

export interface ComposerCommandRun {
  /** 正在执行的指令名；为空表示没有在跑。 */
  running: string | null;
  /** 执行态文案；为空表示不在执行。 */
  label: string | null;
  /** 已耗时（毫秒）；不在执行时为空。 */
  elapsedMs: number | null;
  /** `/mcp` 摊开的服务器清单；为空表示没有可摊开的东西。 */
  servers: McpRuntimeServer[] | null;
  /** 执行一条指令。 */
  run: (name: string) => void;
  /** 收起清单。 */
  close: () => void;
}

export function useComposerCommandRun(
  execute: ((name: string) => Promise<McpCommandReceipt>) | undefined,
  notify: (text: string, tone: 'info' | 'danger') => void,
): ComposerCommandRun {
  const [running, setRunning] = React.useState<RunningCommand | null>(null);
  const [elapsedMs, setElapsedMs] = React.useState<number | null>(null);
  const [servers, setServers] = React.useState<McpRuntimeServer[] | null>(null);
  /** 同步闸门：连点时第二次不进队列。 */
  const busy = React.useRef(false);

  React.useEffect(() => {
    if (running === null) {
      setElapsedMs(null);
      return undefined;
    }
    const tick = () => setElapsedMs(Date.now() - running.startedAtMs);
    tick();
    const timer = window.setInterval(tick, TICK_MS);
    return () => window.clearInterval(timer);
  }, [running]);

  const run = React.useCallback(
    (name: string) => {
      if (execute === undefined || busy.current) return;
      busy.current = true;
      setServers(null);
      setRunning({ name, startedAtMs: Date.now() });
      void execute(name)
        .then((receipt) => {
          notify(receipt.message, receipt.ok ? 'info' : 'danger');
          setServers(receipt.servers ?? null);
        })
        .catch((failure: unknown) => {
          notify(failure instanceof Error ? failure.message : String(failure), 'danger');
          setServers(null);
        })
        .finally(() => {
          busy.current = false;
          setRunning(null);
        });
    },
    [execute, notify],
  );

  const close = React.useCallback(() => setServers(null), []);

  return {
    running: running?.name ?? null,
    label: running === null ? null : commandLabel(running.name),
    elapsedMs,
    servers,
    run,
    close,
  };
}
