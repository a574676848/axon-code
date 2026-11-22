/*
 * 运行态证据的读取节拍。
 *
 * 只在「界面认为这一轮在跑」时读：没有在跑态就没有需要核对的证据，空闲会话的每一拍
 * 都是白跑一次网络。读取周期固定在秒级，够在租约到期后一两拍内发现证据过期，又不至于
 * 让后台服务每分钟多出几十次请求。
 *
 * 失败不弹错：保留最近一次读数，并记下连续失败从哪一刻开始；界面按它把「读不到」
 * 与「读到了但没有执行者」分成两种说明。
 */

import { useEffect, useState } from 'react';

import { conversationSessionLive } from './sessionEndpoints';
import type { SessionLiveReading } from './liveEvidence';

export { liveEvidence } from './liveEvidence';
export type { LiveEvidence, SessionLiveReading } from './liveEvidence';

/** 在跑期间核对证据的周期。 */
export const LIVE_POLL_MS = 5_000;

/** 一份证据与它的所属会话。 */
interface LiveState {
  sessionId: string;
  reading: SessionLiveReading;
}

/**
 * 读当前会话的运行态证据。
 *
 * `active` 为假时停止轮询并清空读数：这一轮已经没有在跑态，旧证据留在手上只会误导。
 */
export function useSessionLive(sessionId: string | null, active: boolean): SessionLiveReading | null {
  const [state, setState] = useState<LiveState | null>(null);

  useEffect(() => {
    if (sessionId === null || !active) return undefined;
    let cancelled = false;
    /** 连续失败的起点；一次成功即清零。 */
    let unreachableSinceMs: number | null = null;
    const load = () => {
      conversationSessionLive(sessionId)
        .then((live) => {
          if (cancelled) return;
          unreachableSinceMs = null;
          setState({ sessionId, reading: { live, receivedAtMs: Date.now(), unreachableSinceMs } });
        })
        .catch(() => {
          if (cancelled) return;
          unreachableSinceMs ??= Date.now();
          setState((previous) => {
            // 一次都没读到过时没有可降级的证据，保持 unknown。
            if (previous === null || previous.sessionId !== sessionId) return previous;
            return {
              sessionId,
              reading: { ...previous.reading, unreachableSinceMs },
            };
          });
        });
    };
    load();
    const timer = window.setInterval(load, LIVE_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [sessionId, active]);

  if (sessionId === null || state === null || state.sessionId !== sessionId) return null;
  return state.reading;
}
