/*
 * 会话累计读数：把网关基线与本视图内的增量合成「累计 token」与「累计耗时」。
 *
 * 历史正文是逐条回放的，界面本来只知道自己翻到过的那些轮次；基线由网关按车道聚合，
 * 随历史页一起取回，因此打开一条历史会话时，累计读数就是这条会话的既有消耗。
 *
 * 两个例外需要单独承担，否则读数会在读者眼皮底下停住或回落：
 * - 基线取回之后才发生的轮次（含正在生成的那一轮）不在基线里，按增量计入；
 * - 打开会话时仍在生成的那一轮，基线里还没有它（账本只在收口时落账），收口后也不会
 *   回填到已经取回的基线，因此把它的标识记下来，整条由增量承担。
 */

import { useEffect, useMemo, useState } from 'react';

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
  /** 打开时仍在跑、因此基线里还没有的那几条轮次。 */
  const [lateTurnIds, setLateTurnIds] = useState<ReadonlySet<string>>(() => new Set());

  useEffect(() => {
    if (baseline === null) return;
    const added: string[] = [];
    for (const turn of turns) {
      if (!baseline.turnIds.has(turn.id)) continue;
      if (turn.status !== 'streaming' && turn.status !== 'pending') continue;
      if (lateTurnIds.has(turn.id)) continue;
      added.push(turn.id);
    }
    if (added.length === 0) return;
    setLateTurnIds((current) => {
      const next = new Set(current);
      for (const id of added) next.add(id);
      return next;
    });
  }, [turns, baseline, lateTurnIds]);

  return useMemo(
    () => sessionCumulative(turns, baseline, lateTurnIds),
    [turns, baseline, lateTurnIds],
  );
}
