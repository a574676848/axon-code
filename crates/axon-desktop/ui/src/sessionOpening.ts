/*
 * 从页面进入会话的公共构造。
 *
 * 目标看板与自动化页都要把「某条后台执行会话」交给主舞台：两条入口共用同一套
 * `LaneSelection` 口径与同一段只读判定，避免各写一份后逐渐漂移。
 */

import type { LaneSelection } from './conversation/targets';

/** 后台驱动会话的只读说明：输入坞禁用时显示在占位位置。 */
export const READ_ONLY_PLACEHOLDER = '目标由后台执行，这里只读';

/** 独立会话（session 目标）由后台驱动，界面只读；车道会话仍由用户提交。 */
export const isReadOnlySession = (selection: Pick<LaneSelection, 'sessionId'> | null): boolean =>
  (selection?.sessionId ?? '').length > 0;

/** 目标执行会话的选择：带上 `sessionId` 才会按 session 目标订阅。 */
export function goalSessionSelection(target: {
  workspaceId: string;
  workspacePath: string;
  sessionId: string;
  title: string;
}): LaneSelection {
  return {
    workspaceId: target.workspaceId,
    workspacePath: target.workspacePath,
    laneId: target.sessionId,
    destination: '',
    title: target.title,
    channel: 'goal',
    sessionId: target.sessionId,
  };
}

/** 会话顶栏的来源返回条：只有从页面进入的后台会话才有。 */
export interface SessionOriginBarState {
  /** 返回文案，例如「返回看板」。 */
  backLabel: string;
  /** 会话标题，用于确认自己看的是哪一件事。 */
  title: string;
  onBack: () => void;
}

/** 按会话来源给出返回条；普通车道会话返回 null，不占高度。 */
export function originBarFor(
  selection: LaneSelection | null,
  navigate: (tab: 'goals' | 'automation') => void,
): SessionOriginBarState | null {
  if (selection === null) return null;
  if (selection.channel === 'goal') {
    return { backLabel: '返回看板', title: selection.title, onBack: () => navigate('goals') };
  }
  if (selection.channel === 'cron') {
    return { backLabel: '返回自动化', title: selection.title, onBack: () => navigate('automation') };
  }
  return null;
}
