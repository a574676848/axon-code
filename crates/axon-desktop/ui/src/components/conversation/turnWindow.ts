/*
 * 正文渲染窗口。
 *
 * 正文按轮次成块渲染，窗口也按轮次切：从最新一轮往前数，数满上限就停，更早的整轮
 * 折算成「更早的 N 条内容已折叠」。
 *
 * 与逐单元切窗口相比，按轮切的意义在于渲染边界：一轮要么整块在窗口里、要么整块不在，
 * 于是未变化的轮次可以整块跳过重建，增量帧只需要花在真正变化的那一轮上。
 */

import type { TurnState } from '../../conversation/transcript/state';

/** 一轮在窗口里的形态。 */
export interface WindowedTurn {
  /** 轮次在完整列表里的下标，用于展示轮次序号。 */
  index: number;
  turn: TurnState;
  /** 这一轮里被窗口折掉的前导单元数；0 表示整轮都在窗口里。 */
  hiddenCells: number;
}

/** 切窗口的结果。 */
export interface TurnWindow {
  /** 窗口内的轮次，顺序与完整列表一致（由早到晚）。 */
  turns: WindowedTurn[];
  /** 被折掉的单元总数，用于那句提示文案。 */
  hiddenCells: number;
}

/**
 * 按单元预算从最新一轮往前切窗口。
 *
 * 预算只数单元，不数轮次：一轮可能有一格，也可能有上百格，按轮数限制挡不住大轮次。
 * 预算之内的一轮整块保留；跨在边界上的一轮只折掉它的前导单元，正文的最新部分始终可见。
 */
export function turnWindow(turns: readonly TurnState[], budget: number): TurnWindow {
  const kept: WindowedTurn[] = [];
  let remaining = Math.max(0, budget);
  let hiddenCells = 0;
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    const count = turn.cells.length;
    if (remaining <= 0) {
      hiddenCells += count;
      continue;
    }
    const hidden = count > remaining ? count - remaining : 0;
    hiddenCells += hidden;
    remaining -= count - hidden;
    kept.push({ index, turn, hiddenCells: hidden });
  }
  kept.reverse();
  return { turns: kept, hiddenCells };
}
