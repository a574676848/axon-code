/*
 * 轮内单元的显示顺序。
 *
 * 正文按到达顺序排列，只有一类单元需要挪位置：运行中追加的用户消息。它不属于
 * 已经发生的过程，夹在思考与工具行之间会被读成那一行的一部分。挪动只对还没被
 * 模型接住的消息成立：一旦接住，后续的正文、思考与工具行就是按到达顺序排在它
 * 后面的，再把它提到轮末会把回答留在上方，读成「回答在前、提问在后」。
 */

import { steerSequence } from './derive';
import { LOCAL_STEER_PREFIX, type TranscriptCell } from './state';

/** 追加消息之后「模型已经接着干活」的单元：有它跟在后面就说明这条消息被接住了。 */
const CONTINUATION_ROLES = new Set<TranscriptCell['role']>([
  'reasoning',
  'tool',
  'assistant',
  'fileChange',
  'history',
]);

/** 运行中追加的用户消息。 */
function appendedWhileRunning(cell: TranscriptCell): boolean {
  return cell.role === 'user' && cell.steer === true;
}

/** 最后一条真实工作的下标；整轮只有气泡与提示时为 -1。 */
function lastWorkedIndex(cells: readonly TranscriptCell[]): number {
  for (let index = cells.length - 1; index >= 0; index -= 1) {
    if (CONTINUATION_ROLES.has(cells[index].role)) return index;
  }
  return -1;
}

/**
 * 这条追加消息是否仍要排到轮末。
 *
 * 判据是「模型有没有接住它」：本地排队标识还在（等注入、未送达、待转投），或者
 * 它之后除了收口提示与分割线之外没有任何工作，都说明它还没被答复；反过来，认领
 * 之后已经有正文、思考或工具行排在它后面，到达顺序就是真实顺序。
 */
function waitsAtTail(cells: readonly TranscriptCell[], index: number, lastWorked: number): boolean {
  const cell = cells[index];
  if (!appendedWhileRunning(cell)) return false;
  if (cell.id.startsWith(LOCAL_STEER_PREFIX)) return true;
  return index > lastWorked;
}

/**
 * 排序：把仍待接手的追加消息按用户敲的先后挪到轮末，其余保持到达顺序。
 *
 * 一轮里没有此类单元时原样返回同一个数组，纯历史轮次因此不会因为这一层而换引用、
 * 让轮次的 memo 边界失效。
 */
export function orderTurnCells(cells: TranscriptCell[]): TranscriptCell[] {
  if (!cells.some(appendedWhileRunning)) return cells;
  const lastWorked = lastWorkedIndex(cells);
  const head: TranscriptCell[] = [];
  const tail: TranscriptCell[] = [];
  cells.forEach((cell, index) => {
    (waitsAtTail(cells, index, lastWorked) ? tail : head).push(cell);
  });
  if (tail.length === 0) return cells;
  if (tail.length > 1) tail.sort((left, right) => steerSequence(left) - steerSequence(right));
  return [...head, ...tail];
}
