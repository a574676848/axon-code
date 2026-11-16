/*
 * 折叠生效。
 *
 * 只有一条判据：除最新一轮之外，能折的都折（见 `shouldFold`）。最新一轮是正在读的
 * 那一段，收起来等于把阅读对象抽走；而其余轮次折起来不损失任何一句话，只把过程
 * 撤下，因此不再需要体量阈值与轮次窗口两套判据。
 *
 * 折叠时该轮的过程单元被撤下，工具输出与推理正文随即失去引用、被回收；用户点开过的
 * 一轮不会被自动收回（`keepExpanded`），每一轮至多折一次，因此这个 effect 自行收敛。
 */

import { useEffect } from 'react';
import { shouldFold, type ConversationAction, type TurnState } from './transcript';

/** 除最新一轮之外，把该折的都折起来。 */
export function useAutoFoldTurns(
  turns: readonly TurnState[],
  dispatch: (action: ConversationAction) => void,
): void {
  useEffect(() => {
    for (let index = 0; index < turns.length; index += 1) {
      const turn = turns[index];
      if (!shouldFold(turn, index, turns.length)) continue;
      dispatch({ type: 'fold-turn', turnId: turn.id });
    }
  }, [dispatch, turns]);
}
