import { useCallback, useMemo } from 'react';
import { isRunning, type SessionRuntimeFacts } from '../conversation/runtimeFacts';
import type { SessionSelection } from '../conversation/useConversation';
import type { NavTab, WorkspaceItem } from '../types';
import { railActivity, type RailActivity } from './railActivity';

/** 收轨态徽标需要的外壳事实，全部由外壳持有，这里只做投影与跳转。 */
export interface UseRailActivityOptions {
  workspaces: WorkspaceItem[];
  sessionMarks?: Record<string, string>;
  runtime: SessionRuntimeFacts | null;
  onSelectSession?: (selection: SessionSelection | null) => void;
  onSelectTab: (tab: NavTab) => void;
}

/** 把徽标聚合与卡片跳转收在一处，Sidebar 只消费结果。 */
export function useRailActivity({
  workspaces,
  sessionMarks,
  runtime,
  onSelectSession,
  onSelectTab,
}: UseRailActivityOptions): {
  activity: RailActivity;
  openSession: (workspaceId: string, sessionId: string) => void;
} {
  // 实时轮次只补当前会话刚起的那一轮，口径与展开态传给 WorkspaceTree 的一致。
  const activity = useMemo(
    () =>
      railActivity(
        workspaces,
        sessionMarks,
        runtime !== null && isRunning(runtime) ? runtime.sessionId : null,
      ),
    [workspaces, sessionMarks, runtime],
  );

  // 折叠态不展开侧栏，只切主区；选中即由外壳清掉该条的未查看标识。
  const openSession = useCallback(
    (workspaceId: string, sessionId: string) => {
      const workspace = workspaces.find((item) => item.id === workspaceId);
      const session = workspace?.sessions.find((item) => item.id === sessionId);
      if (workspace === undefined || session === undefined) return;
      onSelectSession?.({
        workspaceId,
        workspacePath: workspace.path,
        sessionId,
        destination: session.destination,
        title: session.title,
        channel: session.channel,
        pending: session.pending,
        draft: session.draft,
      });
      onSelectTab('chat');
    },
    [workspaces, onSelectSession, onSelectTab],
  );

  return { activity, openSession };
}
