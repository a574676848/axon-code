/*
 * 会话级读数的组合：上下文快照与运行态证据。
 *
 * 两者都是「界面此刻该说什么」的事实来源，且都只经网关读：
 * - 上下文快照给头部占用与换窗进度行，同一份带时间戳的读数；
 * - 运行态证据回答「这一轮到底还在不在跑」，证据过期时界面必须改口。
 *
 * 收在一个 hook 里，是为了让视图层只取一次、只写一行；两个读取节拍各自独立
 * （快照跟轮次与低频重读，证据只在在跑时核对），实现留在各自的 hook 里。
 */

import { useContextSnapshot } from './useContextSnapshot';
import { liveEvidence, useSessionLive } from './useSessionLive';
import type { ContextSnapshotReading } from './contextSnapshot';
import type { LiveEvidence } from './liveEvidence';

/** 视图层需要的两组会话级读数。 */
export interface ConversationObservations {
  /** 上下文快照读数；空表示还没有任何可用读数。 */
  contextSnapshot: ContextSnapshotReading | null;
  /** 运行态证据结论。 */
  evidence: LiveEvidence;
}

/**
 * 组装会话级读数。
 *
 * `revision` 是轮次数量：新的一轮开始或收口都要重读一次快照。证据只在 `working`
 * 为真时核对，因此空闲会话不会因此多出请求。
 */
export function useConversationObservations(
  sessionId: string | null,
  working: boolean,
  revision: number,
): ConversationObservations {
  const contextSnapshot = useContextSnapshot(sessionId, revision);
  const liveReading = useSessionLive(sessionId, working);
  return { contextSnapshot, evidence: liveEvidence(working, liveReading, Date.now()) };
}
