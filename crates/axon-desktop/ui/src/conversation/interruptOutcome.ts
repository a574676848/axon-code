/*
 * 中断回执的结论口径。
 *
 * 服务端回「没有活跃轮次」时，界面不能就此宣布已经收口：索引可能仍把这一轮记成在途，
 * 那正是「提示已经结束、spinner 却不消失」的现场。因此回执为假时必须**重读一次轮次
 * 索引**再决定提示，并且只有索引也说不在跑时才给「已经结束」；索引仍说在途就如实
 * 说明这是界面状态待修复，不静默保持转圈。
 *
 * 这里只产出提示文案，不做任何状态改写：收口是服务端的事实，界面不得单方面把一轮
 * 标成已中断。服务端的显式收口接口属另一条车道。
 */

import type { InterruptReceipt } from '../types';
import { interactionStillRunning } from './historyProjection';

/** 重读轮次索引得到的结论：最新一轮的状态、索引里一条轮次都没有，或这一次没读出来。 */
export type LatestTurnReading =
  | { kind: 'status'; status: string }
  | { kind: 'empty' }
  | { kind: 'unreadable' };

/** 一次中断的结论：给用户的那句话，以及是否为了下结论重读了索引。 */
export interface InterruptOutcome {
  /** 需要提示的结论；无事可说时为 null。 */
  notice: string | null;
  /** 是否重读了轮次索引。 */
  refetched: boolean;
}

/** 回执为假、索引却仍说在途：这是界面状态与服务的分歧，必须说出来。 */
const STALE_INDEX_NOTICE = '服务端无运行轮次，界面状态待修复';
/** 回执与索引一致：这一轮确实已经结束。 */
const SETTLED_NOTICE = '本轮已经结束，无需中断';
/** 回执为假但索引读不出来：如实说结论未经复核，不冒充已确认。 */
const UNVERIFIED_NOTICE = '本轮已经结束，无需中断（轮次索引未能复核）';
/** 中断已受理但本轮还没停下：取消会在下一个安全点生效。 */
const CANCELLATION_NOTICE = '已请求取消，本轮会在安全点收口';

/**
 * 由回执与索引给出提示。
 *
 * 回执为真时不再读索引：服务端已经受理，索引只会比回执更旧。
 */
export async function interruptOutcome(
  receipt: InterruptReceipt,
  readLatestTurn: () => Promise<LatestTurnReading>,
): Promise<InterruptOutcome> {
  if (receipt.interrupted) {
    return {
      notice: receipt.cancellationRequested ? CANCELLATION_NOTICE : null,
      refetched: false,
    };
  }
  const latest = await readLatestTurn();
  if (latest.kind === 'unreadable') return { notice: UNVERIFIED_NOTICE, refetched: true };
  if (latest.kind === 'empty') return { notice: SETTLED_NOTICE, refetched: true };
  return {
    notice: interactionStillRunning(latest.status) ? STALE_INDEX_NOTICE : SETTLED_NOTICE,
    refetched: true,
  };
}

/**
 * 读一次轮次索引，只留下最新那一轮的状态。
 *
 * 索引按时间正序给出这一页里的轮次（最新一条在末尾），因此取末尾那一条；一条都没有
 * 说明这条车道还没有轮次，本轮自然无从谈起「在跑」。读取失败不是「已收口」，而是
 * 「没法复核」，两者在文案上必须分开。
 */
export async function readLatestTurnReading(
  readIndex: () => Promise<readonly { status: string }[]>,
): Promise<LatestTurnReading> {
  try {
    const interactions = await readIndex();
    const latest = interactions[interactions.length - 1];
    return latest === undefined ? { kind: 'empty' } : { kind: 'status', status: latest.status };
  } catch {
    return { kind: 'unreadable' };
  }
}
