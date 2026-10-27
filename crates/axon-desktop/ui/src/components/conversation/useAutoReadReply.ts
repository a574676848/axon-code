import React from 'react';
import type { TranscriptCell, TurnState } from '../../conversation/transcript';
import { speak } from '../../speech';

/*
 * 自动朗读：一轮收口时读这一轮最后一段助手正文。
 *
 * 触发点放在会话外壳而不是每条消息里：流式回复与「收口时补齐整段正文」两种落地
 * 方式都会走到同一处，因此不会漏掉没有增量帧的回复；历史回放挂载时也不会把旧
 * 回复从头念一遍。会话切换那一帧不触发，上一条会话的收口不算新回复。
 */

/** 这一轮最后一段可朗读的助手正文；没有就返回 null。 */
function replyCellOf(
  turn: TurnState | undefined,
): Extract<TranscriptCell, { role: 'assistant' }> | null {
  if (turn === undefined) return null;
  for (let index = turn.cells.length - 1; index >= 0; index -= 1) {
    const cell = turn.cells[index];
    if (cell === undefined || cell.role !== 'assistant' || cell.streaming) continue;
    if (cell.text.trim().length === 0) continue;
    return cell;
  }
  return null;
}

export function useAutoReadReply(
  laneKey: string | null,
  working: boolean,
  turns: readonly TurnState[],
  enabled: boolean,
): void {
  const previous = React.useRef({ laneKey, working });
  React.useEffect(() => {
    const before = previous.current;
    previous.current = { laneKey, working };
    // 刚切会话：这一帧的收口属于上一条会话，不念。
    if (before.laneKey !== laneKey) return;
    // 只在「在跑」翻成「收口」的那一帧念一次。
    if (!before.working || working || !enabled) return;
    const turn = turns[turns.length - 1];
    // 中断与失败留下的半截正文不是一条回复，不念。
    if (turn === undefined || turn.status !== 'completed') return;
    const cell = replyCellOf(turn);
    if (cell === null) return;
    speak(cell.text, { source: cell.id });
  }, [enabled, laneKey, turns, working]);
}
