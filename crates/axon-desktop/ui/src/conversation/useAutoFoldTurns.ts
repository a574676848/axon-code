/*
 * 折叠生效。
 *
 * 两条判据（单轮体量、轮次窗口）都只作用于最近两轮之外的老轮次：发送新一轮时，
 * 上一轮仍是阅读对象，不能在按下的同一刻被收成摘要（见 `isFoldCandidate`）。
 *
 * 折叠时该轮的正文单元被丢弃，工具输出与推理正文随即失去引用、被回收；用户点开过的
 * 一轮不会被自动收回（`keepExpanded`），每一轮至多折一次，因此这个 effect 自行收敛，
 * 不会来回派发。
 */

import { useEffect } from 'react';
import {
  foldedSummary,
  isFoldCandidate,
  shouldEvictFromWindow,
  shouldFold,
  type ConversationAction,
  type TurnState,
} from './transcript';

/** 按体量与窗口两条判据让折叠生效；最近两轮不参与。 */
export function useAutoFoldTurns(
  turns: readonly TurnState[],
  dispatch: (action: ConversationAction) => void,
): void {
  useEffect(() => {
    for (let index = 0; index < turns.length; index += 1) {
      if (!isFoldCandidate(index, turns.length)) continue;
      const turn = turns[index];
      const evicted = shouldEvictFromWindow(turn, index, turns.length);
      if (!evicted && !shouldFold(turn)) continue;
      dispatch({ type: 'fold-turn', turnId: turn.id, summary: foldedSummary(turn) });
    }
  }, [dispatch, turns]);
}
