/**
 * 侧栏树的纯计算：搜索、归档筛选、排序与当前工作区归属。
 *
 * 从 `Sidebar.tsx` 抽出的唯一理由是行数硬限；行为一字未改，仍然是「网关给出的次序
 * 优先、手动顺序档原样沿用、其余按置顶与最近活动重放一遍」这三条。
 */
import type { SessionItem, WorkspaceItem } from '../types';
import type { ArchivedFilter, OrderMode } from './ViewOptionsMenu';

/** 加工展示用的侧栏树所需的输入。 */
export interface SidebarTreeInput {
  workspaces: WorkspaceItem[];
  fullPaths: Record<string, string>;
  query: string;
  archivedFilter: ArchivedFilter;
  orderBy: OrderMode;
  selectedSessionId: string | null;
  activeSessionId: string;
  activeWorkspaceId: string;
}

/** 按搜索、归档筛选与排序规则加工展示数据。 */
export function buildSidebarTree(input: SidebarTreeInput): WorkspaceItem[] {
  const keyword = input.query.trim().toLowerCase();
  const currentSession = input.selectedSessionId ?? input.activeSessionId;
  const currentWorkspace =
    input.workspaces.find((workspace) =>
      workspace.sessions.some((session) => session.id === currentSession),
    )?.id ?? input.activeWorkspaceId;

  const sessionsOf = (workspace: WorkspaceItem): SessionItem[] =>
    workspace.sessions
      .filter((session) => {
        if (input.archivedFilter === 'hide' && session.archived) return false;
        if (input.archivedFilter === 'only' && !session.archived) return false;
        // 未激活会话是「即将开始的会话」，只保留当前这一条：
        // 历史遗留的空会话不该在列表里长期占位，这与既有工作台产品一致。
        if (session.pending && session.id !== currentSession) return false;
        if (keyword.length === 0) return true;
        return (
          session.title.toLowerCase().includes(keyword) ||
          workspace.name.toLowerCase().includes(keyword)
        );
      })
      .sort((left, right) => {
        /*
         * 置顶块优先、其余按最近活动倒序，与网关 `sessions` 清单的排序规则一致。
         *
         * 置顶与最近活动都由运行态投影给出（桌面端不再自存一份状态），因此这里只是把
         * 同一条规则重放一遍，不会算出一个与网关不同的次序；「手动顺序」档保留后端
         * 给出的次序，用户在列表里拖动过之后就以那次拖动为准。
         */
        if (input.orderBy !== 'updated') return 0;
        if (left.pinned !== right.pinned) return left.pinned ? -1 : 1;
        return right.updated_at - left.updated_at;
      });

  return input.workspaces.map((workspace) => ({
    ...workspace,
    active: workspace.id === currentWorkspace,
    sessions: sessionsOf(workspace),
    // 树形分组需要真实路径做父子判定。
    path: input.fullPaths[workspace.id] ?? workspace.path,
  }));
}
