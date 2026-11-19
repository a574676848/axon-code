/**
 * 启动恢复「上次看到哪」：展开上次打开的工作区、打开它最后一条会话。
 *
 * 事实全部来自本地状态文件（`desktop-state.v2.json` 的 `client.lastWorkspaceDir`、
 * `lastSessionId` 与 `expandedWorkspaceIds`），不向服务端推导；服务端的会话清单只按
 * 最近活动排序，它不知道用户上次点开的是哪一条。
 *
 * 只恢复一次：用户之后手动收起工作区或换会话，轮询回来不再抢回选中的那一屏。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import * as ipc from './ipc';
import type { SessionSelection } from './conversation/useConversation';
import { resolveRestoreTarget, workspaceOfDir } from './restoreView';
import type { WorkspaceItem } from './types';

/** 恢复结果：侧栏展开态与外壳选中项共用同一份本地记录。 */
export interface LastViewRestore {
  /** 启动时默认展开的工作区标识。 */
  preferredWorkspaceId: string;
  /** 状态文件记录的展开清单。 */
  expandedWorkspaceIds: string[];
  /** 用户手动开合工作区时回写状态文件。 */
  rememberExpanded: (ids: string[]) => void;
}

/** 读取本地状态并完成一次恢复；`onRestore` 由外壳提供（它持有真实工作区与会话清单）。 */
export function useLastViewRestore(input: {
  workspaces: WorkspaceItem[];
  selection: SessionSelection | null;
  onRestore: (selection: SessionSelection) => void;
}): LastViewRestore {
  const { workspaces, selection, onRestore } = input;
  const lastViewRef = useRef<{ workspaceDir: string | null; sessionId: string | null }>({
    workspaceDir: null,
    sessionId: null,
  });
  const restoredRef = useRef(false);
  const [expandedWorkspaceIds, setExpandedWorkspaceIds] = useState<string[]>([]);

  useEffect(() => {
    let cancelled = false;
    void ipc
      .desktopPreferences()
      .then((preferences) => {
        if (cancelled) return;
        lastViewRef.current = {
          workspaceDir: preferences.lastWorkspaceDir ?? null,
          sessionId: preferences.lastSessionId ?? null,
        };
        setExpandedWorkspaceIds(preferences.expandedWorkspaceIds ?? []);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (restoredRef.current || selection !== null || workspaces.length === 0) return;
    restoredRef.current = true;
    const target = resolveRestoreTarget(workspaces, lastViewRef.current);
    if (target === null) return;
    onRestore({
      workspaceId: target.workspace.id,
      workspacePath: target.workspace.path,
      sessionId: target.session.id,
      destination: target.session.destination,
      title: target.session.title,
      channel: target.session.channel,
      pending: target.session.pending,
      draft: target.session.draft,
    });
  }, [selection, workspaces, onRestore]);

  /* 用户每换一次会话就把「上次看到哪」写回本地状态文件；写失败不打断交互。 */
  const sessionId = selection?.sessionId;
  const workspacePath = selection?.workspacePath;
  useEffect(() => {
    if (sessionId === undefined || workspacePath === undefined) return;
    void ipc.desktopRememberView({ workspaceDir: workspacePath, sessionId }).catch(() => undefined);
  }, [sessionId, workspacePath]);

  const rememberExpanded = useCallback((ids: string[]) => {
    setExpandedWorkspaceIds(ids);
    void ipc.desktopRememberView({ expandedWorkspaceIds: ids }).catch(() => undefined);
  }, []);

  const preferredWorkspaceId = useMemo(
    () =>
      workspaceOfDir(workspaces, lastViewRef.current.workspaceDir)?.id ??
      workspaces.find((item) => item.active)?.id ??
      workspaces[0]?.id ??
      '',
    [workspaces],
  );

  return { preferredWorkspaceId, expandedWorkspaceIds, rememberExpanded };
}
