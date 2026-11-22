/*
 * 会话级三端点的调用绑定：上下文快照、运行态证据与静默轮次收口。
 *
 * 它们只服务会话视图，而全局 `ipc.ts` 已接近单文件行数硬限，因此按域收在这里：
 * 调用形态与那边完全一致，都走同一个 `call`。字段名逐字对齐网关契约。
 */

import { call } from '../backend';
import type { ContextSnapshot } from './contextSnapshot';

/** 快照读取回执；`snapshot` 为空只表示还没有任何可用读数。 */
export interface SessionContextSnapshot {
  sessionId: string;
  snapshot: ContextSnapshot | null;
}

/**
 * 一条会话的运行态证据。
 *
 * `running` 只认带租约且租约未过期的在跑轮次；证据读不出来时调用失败，
 * 界面据此降级，不拿本地推算顶替。
 */
export interface SessionLive {
  sessionId: string;
  running: boolean;
  leaseRemainingMs: number | null;
  lastFrameSequence: number;
  idleForMs: number;
}

/** 显式收口的服务端判据。 */
export type SettleVerdict =
  | 'lease_expired'
  | 'first_token_deadline_exceeded'
  | 'still_running'
  | 'no_in_flight_turn'
  | 'already_settled';

/** 显式收口回执。 */
export interface SettleSilentTurnReceipt {
  settled: boolean;
  verdict: SettleVerdict;
}

/** 读取一条会话的上下文快照。 */
export const conversationContextSnapshot = (sessionId: string): Promise<SessionContextSnapshot> =>
  call('conversation_context_snapshot', { sessionId });

/** 读取一条会话的运行态证据。 */
export const conversationSessionLive = (sessionId: string): Promise<SessionLive> =>
  call('conversation_session_live', { sessionId });

/** 请服务端显式收口一条静默轮次；界面不得自己改状态。 */
export const conversationSettleSilentTurn = (
  sessionId: string,
  interactionId: string,
): Promise<SettleSilentTurnReceipt> =>
  call('conversation_settle_silent_turn', { sessionId, interactionId });
