/*
 * 运行态证据的读取节拍。
 *
 * 只在「界面认为这一轮在跑」时读：没有在跑态就没有需要核对的证据，空闲会话的每一拍
 * 都是白跑一次网络。读取周期固定在秒级，够在租约到期后一两拍内发现证据过期，又不至于
 * 让后台服务每分钟多出几十次请求。
 *
 * 失败不弹错：保留最近一次读数，并记下连续失败从哪一刻开始；界面按它把「读不到」
 * 与「读到了但没有执行者」分成两种说明。
 *
 * 每次重新进入在跑态都把上一轮的读数撤下并重记起点：提交消息的那一刻界面已经在跑，
 * 而服务端常常还没认领这一轮，拿着两小时前那份读数去算流逝时间，只会算出一个假的过期。
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
  /** 本轮读数；重新进入在跑态后、第一份证据到手前为空。 */
  reading: SessionLiveReading | null;
}

/**
 * 读当前会话的运行态证据。
 *
 * `active` 为假时停止轮询并对外返回空读数（内部状态留到下次进入在跑态时重置）：这一轮
 * 已经没有在跑态，旧证据留在手上只会误导。
 */
export function useSessionLive(sessionId: string | null, active: boolean): SessionLiveReading | null {
  const [state, setState] = useState<LiveState | null>(null);

  useEffect(() => {
    if (sessionId === null || !active) return undefined;
    let cancelled = false;
    /** 连续失败的起点；一次成功即清零。 */
    let unreachableSinceMs: number | null = null;
    /** 本轮工作态起点：同一轮里所有读数共用它，换一轮就重取。 */
    const activeSinceMs = Date.now();
    // 先撤下上一轮的读数：新的一轮还没有任何证据，不该继承旧的流逝时间。
    setState({ sessionId, reading: null });
    const load = () => {
      conversationSessionLive(sessionId)
        .then((live) => {
          if (cancelled) return;
          unreachableSinceMs = null;
          setState({
            sessionId,
            reading: { live, receivedAtMs: Date.now(), unreachableSinceMs, activeSinceMs },
          });
        })
        .catch(() => {
          if (cancelled) return;
          unreachableSinceMs ??= Date.now();
          setState((previous) => {
            // 一次都没读到过时没有可降级的证据，保持 unknown。
            if (previous === null || previous.sessionId !== sessionId || previous.reading === null) {
              return previous;
            }
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

  if (sessionId === null || !active || state === null || state.sessionId !== sessionId) return null;
  return state.reading;
}
