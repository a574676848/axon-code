import { useCallback, useEffect, useRef, useState } from 'react';

import * as ipc from './ipc';
import type { WorkspaceItem } from './types';

/*
 * 工作区与会话清单的唯一读取点。
 *
 * 会话可能由其它渠道产生（例如 TUI 新开一条），界面必须能自己发现，因此有一条静默
 * 轮询。这条轮询此前在侧栏里也有一份：同一份列表每 15 秒被读两次，两边的失败结论还
 * 可能一先一后。现在取数、轮询与失败结论都收在这里，外壳持有、侧栏消费。
 *
 * 失败分两档：轮询与首读静默（连接问题由统一探活呈现），只有用户主动点重试时
 * 才把结论挂出来，因此 return 的失败原因只给重试路径用。
 */

export interface WorkspacesFeedOptions {
  /** 当前是否已连上：未连上时不发请求，等连接结论落地。 */
  connected: () => boolean;
  /** 需要立即重读的外部变化（选中会话、连接状态）；变化时补一次静默刷新。 */
  refreshOn: string;
  /** 静默轮询间隔。 */
  intervalMs: number;
}

export interface WorkspacesFeed {
  workspaces: WorkspaceItem[];
  /** 是否已经成功读过一次：连上但还没读到数据时给骨架而不是空态。 */
  loaded: boolean;
  /** 用户主动重试后的失败原因；轮询失败不进这里。 */
  error: string | null;
  /** 单次动作（新建、置顶、归档）返回的新清单写回这里。 */
  setWorkspaces: (list: WorkspaceItem[]) => void;
  /** 重新读取；返回失败原因，成功为空。 */
  retry: () => Promise<string | null>;
}

export function useWorkspacesFeed({
  connected,
  refreshOn,
  intervalMs,
}: WorkspacesFeedOptions): WorkspacesFeed {
  const [workspaces, setWorkspaces] = useState<WorkspaceItem[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** 连接结论与读取函数都经 ref 取当下值：它们换引用不该重启定时器。 */
  const connectedRef = useRef(connected);
  connectedRef.current = connected;

  const load = useCallback(async (reportFailure: boolean): Promise<string | null> => {
    if (!connectedRef.current()) return null;
    try {
      setWorkspaces(await ipc.fetchWorkspaces());
      setLoaded(true);
      setError(null);
      return null;
    } catch (failure) {
      const message = failure instanceof Error ? failure.message : '读取工作区失败';
      if (reportFailure) setError(message);
      return message;
    }
  }, []);

  const refresh = useCallback(() => load(false), [load]);
  const retry = useCallback(() => load(true), [load]);
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;

  // 外部变化（选中会话、连接刚就绪）后补一次：静默，不动界面状态。
  useEffect(() => {
    void refreshRef.current();
  }, [refreshOn]);

  // 静默轮询：只换数据，不动界面状态，因此不会打断正在输入或翻历史的用户。
  useEffect(() => {
    const timer = window.setInterval(() => void refreshRef.current(), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);

  return { workspaces, loaded, error, setWorkspaces, retry };
}
