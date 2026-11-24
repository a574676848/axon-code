import React from 'react';
import * as goalIpc from '../../goalIpc';
import type { GoalCardView } from '../../goalTypes';

/*
 * 本会话创建的目标清单。
 *
 * 「谁建的」由目标的 `createdFromSessionId` 回答，因此清单只列这条会话自己建过的
 * 目标，别人的目标不会混进来。会话里随时可能新建目标，所以按固定间隔静默刷新；
 * 读失败保留上一次清单，它与正文无关，不该打断阅读。
 */

/** 静默刷新间隔。 */
const GOAL_LIST_REFRESH_MS = 15_000;

export function useSessionGoals(
  sessionId: string | null,
  workspaceId: string | null,
): GoalCardView[] {
  const [goals, setGoals] = React.useState<GoalCardView[]>([]);
  React.useEffect(() => {
    if (sessionId === null) {
      setGoals([]);
      return undefined;
    }
    let cancelled = false;
    const refresh = () => {
      void goalIpc
        .goalList(undefined, workspaceId)
        .then((view) => {
          if (cancelled) return;
          setGoals(view.goals.filter((goal) => goal.createdFromSessionId === sessionId));
        })
        .catch(() => {
          // 读不到就保留上一次清单。
        });
    };
    refresh();
    const timer = window.setInterval(refresh, GOAL_LIST_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [sessionId, workspaceId]);
  return goals;
}
