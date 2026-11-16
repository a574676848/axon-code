/*
 * 侧栏连接行上的辅助读数：MCP 台数与技能条数。
 *
 * 这两项原本挂在指标条末尾，现在收进侧栏底栏的连接行（档案名之后、延迟读数之前），
 * 因为它们是「这台机器上此刻挂着什么」的事实，与当前打开哪条会话无关。指标条只留
 * 这条会话自己会变的读数。
 *
 * 翻译规则不另立一份：MCP 的台数与异常口径仍由 mcpReading 给出，技能条数与设置 › 技能页
 * 读同一份已安装清单。这里只决定「该出现哪几段、按什么顺序、用什么色调」。
 */

import { mcpSegmentText, mcpSegmentTitle, type McpHeaderReading } from './mcpReading';

/** 侧栏读数只分两档：正常取三级文本色，MCP 异常取危险色。 */
export type SidebarReadoutTone = 'default' | 'critical';

export interface SidebarReadout {
  kind: 'mcp' | 'skill';
  /** 读数的短写，例如 `MCP 3`、`技能 12`。 */
  text: string;
  /** 悬停与无障碍名称的全文。 */
  title: string;
  tone: SidebarReadoutTone;
}

/**
 * 组装侧栏连接行的辅助读数。
 *
 * 顺序固定为 MCP 在前、技能在后：MCP 是外部工具服务，技能是本机能力，两者都紧跟档案名。
 * 与指标条同一套出现条件：MCP 没有服务器且读不到结论时不占位，技能清单未就绪或零条时
 * 同样不占位，绝不拿 0 占宽度。
 */
export function sidebarReadouts(
  mcp: McpHeaderReading | null | undefined,
  skillCount: number | null | undefined,
): SidebarReadout[] {
  const readouts: SidebarReadout[] = [];
  if (mcp !== null && mcp !== undefined) {
    readouts.push({
      kind: 'mcp',
      text: mcpSegmentText(mcp),
      title: mcpSegmentTitle(mcp),
      tone: mcp.failedCount > 0 || mcp.error !== null ? 'critical' : 'default',
    });
  }
  if (skillCount !== null && skillCount !== undefined && skillCount > 0) {
    readouts.push({
      kind: 'skill',
      text: `技能 ${skillCount}`,
      title: `共 ${skillCount} 项已安装技能`,
      tone: 'default',
    });
  }
  return readouts;
}
