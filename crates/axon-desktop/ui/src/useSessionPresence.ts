/*
 * 选中会话的存在性守护：删除与归档都要把主区带回首屏。
 *
 * 侧栏自己的归档动作会立刻交出空选择；这里兜住其余通道（命令行、TUI、另一个窗口）。
 * 判据只认「同一条会话的形态起了变化」，因此打开归档夹里的会话属于浏览，不会被弹走；
 * 清单还没读到这一格时也不下结论，读失败不会把人误弹回首屏。
 */

import { useEffect, useRef } from 'react';

import type { SessionSelection } from './conversation/useConversation';
import { shouldLeaveSession, type SessionPresence } from './sessionPresence';
import type { WorkspaceItem } from './types';

/**
 * 按会话清单观察选中会话的形态，命中删除或归档就交出空选择。
 *
 * 传进来的 `selectSession` 必须是稳定引用（外壳里是 `useCallback`）：它是 effect 的依赖，
 * 每次渲染换身份会让观察每帧重跑一次。
 */
export function useSessionPresence(
  selection: SessionSelection | null,
  workspaces: readonly WorkspaceItem[],
  selectSession: (next: SessionSelection | null) => void,
): void {
  const seenRef = useRef<SessionPresence | null>(null);
  useEffect(() => {
    if (selection === null) {
      seenRef.current = null;
      return;
    }
    const workspace = workspaces.find((item) => item.id === selection.workspaceId);
    // 这一格不在清单里（尚未刷新成功、或工作区没有注册到侧栏）：这一次不下结论。
    if (workspace === undefined) return;
    const listed = workspace.sessions.find((item) => item.id === selection.sessionId);
    const listing = listed === undefined ? null : { archived: listed.archived === true };
    if (shouldLeaveSession(seenRef.current, selection.sessionId, listing)) {
      seenRef.current = null;
      selectSession(null);
      return;
    }
    // 只有真的在清单里见过的会话才记形态：目标、子代理、定时任务永远不进这份清单。
    if (listed !== undefined) {
      seenRef.current = { sessionId: selection.sessionId, archived: listed.archived === true };
    }
  }, [selection, selectSession, workspaces]);
}
