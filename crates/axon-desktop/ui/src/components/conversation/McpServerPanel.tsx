import React from 'react';

import type { McpRuntimeServer } from '../../mcpTypes';
import { CloseIcon } from '../../icons';
import { IconButton } from '../ui/controls';
import { Text } from '../ui/surfaces';
import { Z_MENU } from './chatChrome';

/*
 * `/mcp` 的服务器清单：加载成功的列出工具名，没加载起来的列出原因。
 *
 * 它是一次指令的回执面板，不是常驻页面：贴在输入坞上沿、与输入坞同宽，展开与收起
 * 都不改输入坞版面（与候选菜单同一套做法）。失败项排在前面，因为那才是需要动手的几条。
 *
 * 几何与色值全部取令牌：卡片 surface + 发丝描边 + 14px 圆角，名称 14px，工具名 12px 等宽。
 */

/** 面板正文高度上限：再长就在面板内部滚动，不把输入坞顶走。 */
const MAX_BODY_HEIGHT = 260;

export interface McpServerPanelProps {
  servers: readonly McpRuntimeServer[];
  onClose: () => void;
}

/** 失败项在前：需要动手处理的先说。 */
function ordered(servers: readonly McpRuntimeServer[]): McpRuntimeServer[] {
  return [...servers].sort((left, right) => {
    if (left.failed !== right.failed) return left.failed ? -1 : 1;
    return left.serverId.localeCompare(right.serverId);
  });
}

const McpServerRow: React.FC<{ server: McpRuntimeServer }> = ({ server }) => {
  const failure =
    server.failure !== null && server.failure !== undefined && server.failure.length > 0
      ? server.failure
      : '未注册任何可用工具';
  return (
    <div className="flex flex-col gap-0.5 py-1.5">
      <div className="flex items-baseline gap-2 min-w-0">
        <Text size="row" strong truncate className="min-w-0">
          {server.serverId}
        </Text>
        {server.failed ? (
          <Text size="meta" tone="danger" className="shrink-0">
            加载失败
          </Text>
        ) : (
          <Text size="meta" tone="tertiary" className="shrink-0 tabular-nums">
            {server.toolCount} 个工具
          </Text>
        )}
      </div>
      {server.failed ? (
        <Text size="meta" tone="secondary" className="break-words">
          {failure}
        </Text>
      ) : (
        <Text size="code" mono tone="tertiary" className="break-words line-clamp-2 no-autospace">
          {server.tools.join('、')}
        </Text>
      )}
    </div>
  );
};

export const McpServerPanel: React.FC<McpServerPanelProps> = ({ servers, onClose }) => {
  const rows = ordered(servers);
  const failed = rows.filter((server) => server.failed).length;
  const loaded = rows.length - failed;
  return (
    <section
      aria-label="MCP 服务器"
      className={`absolute bottom-full left-0 right-0 mb-2 ${Z_MENU} rounded-md hairline border-border bg-surface shadow-panel`}
    >
      <header className="flex items-center gap-2 px-3 pt-2 pb-1">
        <Text size="row" strong className="min-w-0 truncate">
          MCP 服务器
        </Text>
        <Text size="meta" tone="tertiary" className="shrink-0 tabular-nums">
          {loaded} 台正常{failed > 0 ? ` · ${failed} 台异常` : ''}
        </Text>
        <span className="flex-1" />
        <IconButton label="关闭 MCP 清单" onClick={onClose}>
          <CloseIcon size={14} />
        </IconButton>
      </header>
      <div
        className="scroll-visible divide-y divide-border-hairline px-3 pb-1 overflow-y-auto"
        style={{ maxHeight: MAX_BODY_HEIGHT }}
      >
        {rows.map((server) => (
          <McpServerRow key={server.serverId} server={server} />
        ))}
      </div>
    </section>
  );
};
