/*
 * 中断回执的结论口径。
 *
 * 服务端回「没有活跃轮次」时，界面不能就此宣布已经收口：索引可能仍把这一轮记成在途，
 * 那正是「提示已经结束、spinner 却不消失」的现场。因此回执为假时必须**重读一次轮次
 * 索引**再决定提示，并且只有索引也说不在跑时才给「已经结束」；索引仍说在途就如实
 * 说明这是界面状态待修复，并把收口交给服务端。
 *
 * 收口只能落在服务端：界面不得单方面把一轮标成已中断，否则等于让界面撒谎。这里因此
 * 在索引仍说在途时调用服务端的显式收口接口，并按它给出的判据说人话。
 */

import type { InterruptReceipt } from '../types';
import { interactionStillRunning } from './historyProjection';
import type { SettleSilentTurnReceipt } from './sessionEndpoints';

/** 重读轮次索引得到的结论：最新一轮的状态与标识、索引里一条轮次都没有，或这一次没读出来。 */
export type LatestTurnReading =
  | { kind: 'status'; status: string; interactionId: string }
  | { kind: 'empty' }
  | { kind: 'unreadable' };

/** 一次中断的结论：给用户的那句话，以及为了下结论做了哪些动作。 */
export interface InterruptOutcome {
  /** 需要提示的结论；无事可说时为 null。 */
  notice: string | null;
  /** 是否重读了轮次索引。 */
  refetched: boolean;
  /** 是否请求了服务端显式收口。 */
  settleRequested: boolean;
  /** 服务端判据；没有请求收口时为空。 */
  verdict: string | null;
}

/** 回执与索引一致：这一轮确实已经结束。 */
const SETTLED_NOTICE = '本轮已经结束，无需中断';
/** 回执为假但索引读不出来：如实说结论未经复核，不冒充已确认。 */
const UNVERIFIED_NOTICE = '本轮已经结束，无需中断（轮次索引未能复核）';
/** 中断已受理但本轮还没停下：取消会在下一个安全点生效。 */
const CANCELLATION_NOTICE = '已请求取消，本轮会在安全点收口';
/** 索引说在途，但服务端收口接口不可用：不假装已经收口。 */
const SETTLE_UNAVAILABLE_NOTICE = '索引仍显示这一轮在途，但后台没有可用的收口接口，请重启后台服务后再试';

/** 服务端判据到文案：只说服务端真的给出的结论。 */
export function settleVerdictNotice(verdict: string): string {
  switch (verdict) {
    case 'lease_expired':
      return '已按服务端判定标记为已中断：这一轮的租约已经过期';
    case 'first_token_deadline_exceeded':
      return '已按服务端判定标记为已中断：首次响应超时';
    case 'still_running':
      return '服务端仍在跑这一轮，未做任何标记';
    case 'no_in_flight_turn':
      return '服务端已没有在途轮次，这一轮本来就已结束';
    case 'already_settled':
      return '这一轮此前已经收口，未重复标记';
    default:
      return `服务端结论：${verdict}`;
  }
}

/** 请求服务端收口；返回 null 表示这套后台不提供该接口。 */
export type SettleRequest = (
  interactionId: string,
) => Promise<SettleSilentTurnReceipt | null>;

/**
 * 由回执与索引给出提示。
 *
 * 回执为真时不再读索引：服务端已经受理，索引只会比回执更旧。
 */
export async function interruptOutcome(
  receipt: InterruptReceipt,
  readLatestTurn: () => Promise<LatestTurnReading>,
  settle: SettleRequest,
): Promise<InterruptOutcome> {
  if (receipt.interrupted) {
    return {
      notice: receipt.cancellationRequested ? CANCELLATION_NOTICE : null,
      refetched: false,
      settleRequested: false,
      verdict: null,
    };
  }
  const latest = await readLatestTurn();
  if (latest.kind === 'unreadable') {
    return { notice: UNVERIFIED_NOTICE, refetched: true, settleRequested: false, verdict: null };
  }
  if (latest.kind === 'empty') {
    return { notice: SETTLED_NOTICE, refetched: true, settleRequested: false, verdict: null };
  }
  if (!interactionStillRunning(latest.status)) {
    return { notice: SETTLED_NOTICE, refetched: true, settleRequested: false, verdict: null };
  }
  // 索引仍说在途：收口必须落在服务端，界面只负责转达与服务端判据的文案。
  let settled: SettleSilentTurnReceipt | null = null;
  try {
    settled = await settle(latest.interactionId);
  } catch {
    settled = null;
  }
  if (settled === null) {
    return {
      notice: SETTLE_UNAVAILABLE_NOTICE,
      refetched: true,
      settleRequested: false,
      verdict: null,
    };
  }
  return {
    notice: settleVerdictNotice(settled.verdict),
    refetched: true,
    settleRequested: true,
    verdict: settled.verdict,
  };
}

/**
 * 读一次轮次索引，只留下最新那一轮。
 *
 * 索引按时间正序给出这一页里的轮次（最新一条在末尾），因此取末尾那一条；一条都没有
 * 说明这条车道还没有轮次，本轮自然无从谈起「在跑」。读取失败不是「已收口」，而是
 * 「没法复核」，两者在文案上必须分开。
 */
export async function readLatestTurnReading(
  readIndex: () => Promise<readonly { status: string; interactionId: string }[]>,
): Promise<LatestTurnReading> {
  try {
    const interactions = await readIndex();
    const latest = interactions[interactions.length - 1];
    return latest === undefined
      ? { kind: 'empty' }
      : { kind: 'status', status: latest.status, interactionId: latest.interactionId };
  } catch {
    return { kind: 'unreadable' };
  }
}
