/*
 * 会话累计读数：把网关基线与本视图内的增量合成「累计 token」与「累计耗时」。
 *
 * 历史正文是按页回放的，界面本来只知道自己翻到过的那些轮次；基线由网关按车道聚合，
 * 随历史页一起取回，因此打开一条历史会话时，累计读数就是这条会话的既有消耗。
 *
 * 两段合成各管一段：基线负责页里已经收口的轮次，本地增量负责基线取回之后才发生的轮次
 * （含正在生成的那一轮，读数因此边跑边涨）。基线里仍标着运行中的轮次由增量承担：
 * 账本只在收口时落账，它们不会回填到已经取回的基线里。
 */

import { useMemo } from 'react';

import {
  sessionCumulative,
  type SessionCumulative,
  type SessionTokenBaseline,
} from './sessionUsage';
import type { TurnState } from './transcript/state';

/** 整条会话的累计 token 与累计耗时；基线未就绪时为空。 */
export function useSessionCumulative(
  turns: readonly TurnState[],
  baseline: SessionTokenBaseline | null,
): SessionCumulative | null {
  return useMemo(() => sessionCumulative(turns, baseline), [turns, baseline]);
}
