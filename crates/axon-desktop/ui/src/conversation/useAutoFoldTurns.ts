/*
 * 折叠生效。
 *
 * 两条判据叠加，与 TUI 的 cell history GC 同一口径：
 *
 * - **体量**：单轮正文过大（工具输出、推理、diff）就折，正在看的一轮除外；
 * - **窗口**：只保留最近若干轮**展开**，更早的一律压成一行摘要。
 *
 * 折叠时该轮的正文单元被丢弃，工具输出与推理正文随即失去引用、被回收；用户点开过的
 * 一轮不会被自动收回（`keepExpanded`），每一轮至多折一次，因此这个 effect 自行收敛，
 * 不会来回派发。
 */

import { useEffect } from 'react';
import {
  foldedSummary,
  shouldEvictFromWindow,
  shouldFold,
  type ConversationAction,
  type TurnState,
} from './transcript';

/** 按体量与窗口两条判据让折叠生效。 */
export function useAutoFoldTurns(
  turns: readonly TurnState[],
  dispatch: (action: ConversationAction) => void,
): void {
  useEffect(() => {
    for (let index = 0; index < turns.length - 1; index += 1) {
      const turn = turns[index];
      const evicted = shouldEvictFromWindow(turn, index, turns.length);
      if (!evicted && !shouldFold(turn)) continue;
      dispatch({ type: 'fold-turn', turnId: turn.id, summary: foldedSummary(turn) });
    }
  }, [dispatch, turns]);
}
