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
import { useSilentTurnSettle } from './useSilentTurnSettle';
import type { ContextSnapshotReading } from './contextSnapshot';
import type { LiveEvidence } from './liveEvidence';
import type { TurnState } from './transcript';

/** 视图层需要的两组会话级读数。 */
export interface ConversationObservations {
  /** 上下文快照读数；空表示还没有任何可用读数。 */
  contextSnapshot: ContextSnapshotReading | null;
  /** 运行态证据结论；判过期时已经与服务端核对过一轮。 */
  evidence: LiveEvidence;
}

/**
 * 组装会话级读数。
 *
 * `revision` 是轮次数量：新的一轮开始或收口都要重读一次快照。证据只在 `working`
 * 为真时核对，因此空闲会话不会因此多出请求。
 * `configuredWindow` 是当前生效模型的窗口（配置事实），仅用于补快照缺失的分母。
 * `turns` 与 `onSettled` 供证据过期时自动收口：身份由仍在跑的那一轮给出，
 * 结论由服务端给出，本 hook 只把两者接起来。
 */
export function useConversationObservations(
  sessionId: string | null,
  working: boolean,
  revision: number,
  configuredWindow: number | null = null,
  turns: readonly TurnState[] = [],
  onSettled: (interactionId: string, verdict: string) => void = () => undefined,
): ConversationObservations {
  const contextSnapshot = useContextSnapshot(sessionId, revision, configuredWindow);
  const liveReading = useSessionLive(sessionId, working);
  const evidence = liveEvidence(working, liveReading, Date.now());
  /** 仍在跑的那一轮：证据过期时按它的 Interaction 标识请服务端收口。 */
  let openTurn: TurnState | undefined;
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    if (turn.status === 'pending' || turn.status === 'streaming') {
      openTurn = turn;
      break;
    }
  }
  const resolved = useSilentTurnSettle({
    sessionId,
    evidence,
    interactionId: openTurn?.interactionId ?? null,
    onSettled,
  });
  return { contextSnapshot, evidence: resolved };
}
