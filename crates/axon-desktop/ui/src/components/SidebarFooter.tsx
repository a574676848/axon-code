/*
 * 侧栏底栏：连接入口一行，设置行与「新版本」入口一行。
 *
 * 从 `Sidebar` 按职责拆出：底栏讲的是「这台机器此刻挂着什么、连的是哪份档案」，
 * 与会话列表的生命周期无关，因此它自己是一个组件，`Sidebar` 只把读数递进来。
 *
 * 连接胶囊自占一行：设置行右端已被「新版本」入口占用，挤进同一行会把两个入口压成
 * 一条，而胶囊是恒常驻的档案入口，不该跟着更新检测的结论出现或消失。
 */

import React from 'react';
import { SettingsIcon } from '../icons';
import type { McpHeaderReading } from '../conversation/mcpReading';
import { Text } from './ui/surfaces';
import { ROW_ICON_COLUMN, ROW_STATE } from './SidebarRows';
import { ConnectionCapsule } from './ConnectionCapsule';
import { UpdateHint } from './UpdateHint';

interface SidebarFooterProps {
  /** MCP 运行态读数；由外壳持有唯一一份，这里只负责呈现。 */
  mcpReading: McpHeaderReading | null;
  /** 已安装技能条数；清单未就绪时为空。 */
  skillCount: number | null;
  /** 打开设置弹窗。 */
  onOpenSettings: () => void;
  /** 打开设置 › 连接。 */
  onOpenConnection: () => void;
}

export const SidebarFooter: React.FC<SidebarFooterProps> = ({
  mcpReading,
  skillCount,
  onOpenSettings,
  onOpenConnection,
}) => (
  <div className="shrink-0 px-3 py-2 space-y-1">
    <ConnectionCapsule
      onManage={onOpenConnection}
      mcpReading={mcpReading}
      skillCount={skillCount}
    />
    <div className="flex items-center gap-2">
      <button
        type="button"
        onClick={onOpenSettings}
        className={`${ROW_STATE} hover:bg-hover flex-1 min-w-0 h-8 text-left`}
      >
        <span className={`${ROW_ICON_COLUMN} text-text-tertiary`}>
          <SettingsIcon size={14} />
        </span>
        <Text size="body" className="leading-5" truncate>
          设置
        </Text>
      </button>
      <UpdateHint />
    </div>
  </div>
);
