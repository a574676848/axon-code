import React from 'react';
import * as ipc from '../../ipc';

/*
 * 活作业名册的对账。
 *
 * 转录状态只是作业启动那一刻的快照，作业收口后那一行不会自己更新。归档判断拿
 * 名册当权威，避免把已经跑完的后台任务算成「还在运行」。
 */

/** 名册对账间隔。 */
const JOB_ROSTER_REFRESH_MS = 10_000;

export function useLiveJobIds(sessionId: string | null): Set<string> | null {
  const [liveJobIds, setLiveJobIds] = React.useState<Set<string> | null>(null);
  React.useEffect(() => {
    if (sessionId === null) {
      setLiveJobIds(null);
      return undefined;
    }
    let cancelled = false;
    const refresh = () => {
      void ipc
        .conversationSessionJobs(sessionId)
        .then((roster) => {
          if (!cancelled) setLiveJobIds(new Set(roster.jobIds));
        })
        .catch(() => {
          // 对账失败保留上一次结论：宁可多提示一次，也不要误判成没有任务。
        });
    };
    refresh();
    const timer = window.setInterval(refresh, JOB_ROSTER_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [sessionId]);
  return liveJobIds;
}
