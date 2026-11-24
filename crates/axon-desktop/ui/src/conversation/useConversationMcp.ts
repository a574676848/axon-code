/*
 * 会话里的 MCP 接线：指标条要的读数与输入坞要的指令执行，在这里合成一份。
 *
 * 两处消费的是同一次取数与同一份翻译（mcpReading），因此放在一处，避免指标条与
 * `/mcp` 各自读一次、各自算一套「几台异常」。
 */

import React from 'react';

import { mcpHeaderReading, type McpCommandReceipt, type McpHeaderReading } from './mcpReading';
import { useMcpRuntime } from './mcpRuntime';

export interface ConversationMcp {
  /** 指标条那一段的读数；不该画时为 null。 */
  reading: McpHeaderReading | null;
  /** 执行一条内置指令并返回回执。 */
  run: (name: string) => Promise<McpCommandReceipt>;
}

export function useConversationMcp(cwd: string | null): ConversationMcp {
  const runtime = useMcpRuntime(cwd);

  const run = React.useCallback(
    (name: string): Promise<McpCommandReceipt> =>
      name === 'mcp'
        ? runtime.reload()
        : Promise.resolve({ ok: false, message: `不支持的内置指令：/${name}` }),
    [runtime.reload],
  );

  return { reading: mcpHeaderReading(runtime.status, runtime.error), run };
}
