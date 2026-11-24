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

/**
 * 历史轮最多同时保留多少条。
 *
 * 折叠轮不吃单元预算（见下），但不能因此没有上限：翻十页就是两百条，每条都是一个
 * 真实 DOM 块，带用户气泡、过程控件与结论正文。超出部分按原有口径折成
 * 「更早的 N 条内容已折叠」。
 */
const HISTORY_TURN_LIMIT = 120;

/** 一轮在窗口里的形态。 */
export interface WindowedTurn {
  /** 轮次在完整列表里的下标，用于展示轮次序号。 */
  index: number;
  turn: TurnState;
  /** 这一轮里被窗口折掉的前导单元数；0 表示整轮都在窗口里。 */
  hiddenCells: number;
}

/**
 * 最新一轮最多直接展开多少行过程行。
 *
 * 运行中的那一轮是全站唯一会持续长高的正文：工具调用一行一行往上堆，几十行之后最新
 * 那一段被推到视野之外，而读者要看的恰恰是最近几行。限 10 行既保住「正在发生什么」，
 * 也让这一轮的 DOM 行数有上界。
 */
export const LATEST_PROCESS_ROW_LIMIT = 10;

/**
 * 最新一轮里该收起的过程行下标（升序）。
 *
 * 只数过程行（思考、执行、工具、改动、历史轮），叙述与用户气泡一律不参与：收起过程
 * 不等于把这一轮压成一行，`叙述 A → 工具 → 叙述 B` 收起后仍读作
 * `叙述 A → 叙述 B`，被收起的只是中间那些过程行。
 *
 * 返回的下标里第一个就是那句「更早的 N 行过程已收起」的落点，其余在展开时回到原位。
 */
export function hiddenProcessRows(
  rows: readonly { process: boolean }[],
  limit: number = LATEST_PROCESS_ROW_LIMIT,
): number[] {
  const processIndices: number[] = [];
  for (let index = 0; index < rows.length; index += 1) {
    if (rows[index].process) processIndices.push(index);
  }
  const keep = Math.max(0, limit);
  if (processIndices.length <= keep) return [];
  return processIndices.slice(0, processIndices.length - keep);
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
  let historyKept = 0;
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    /*
     * 折叠轮不吃单元预算。
     *
     * 它只有气泡、控件与结论三段，却是这一轮唯一的内容入口：按预算折掉之后只剩一句
     * 「更早的 N 条内容已折叠」，连点开的地方都没有。条数上限另行约束，因此这里
     * 只数轮数。
     */
    if (turn.folded) {
      if (historyKept >= HISTORY_TURN_LIMIT) {
        hiddenCells += turn.cells.length;
        continue;
      }
      historyKept += 1;
      kept.push({ index, turn, hiddenCells: 0 });
      continue;
    }
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
