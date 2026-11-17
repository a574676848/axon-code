/*
 * 轮内单元的显示顺序。
 *
 * 正文按到达顺序排列，只有两类单元需要挪位置：
 *
 * - 收尾分割线永远是这一轮的最后一行。它在生成过程中就会立起来（用量帧一到就有读数），
 *   此后到达的工具行与正文按插入顺序排在它后面，不挪回来就会读成
 *   「用户 → 思考 → 分割线 → 工具 → 正文」：分割线夹在过程中间，等于把一句话截成两段。
 * - 运行中追加的用户消息不属于已经发生的过程，夹在思考与工具行之间会被读成那一行的
 *   一部分。挪动只对还没被模型接住的消息成立：一旦接住，后续工作就排在它后面，
 *   再把它提到轮末会把回答留在上方。
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
 * 排序：分割线跟在全部工作之后，仍待接手的追加消息排在它之后，其余保持到达顺序。
 *
 * 分割线这一轮工作到此为止，因此它必须在思考、工具与正文之后；而还没被接住的追加
 * 消息是「本轮结束后要转投的几句」，按 §6.1.3 排在收尾提示之后，不能被分割线压到
 * 前面去。
 *
 * 一轮里没有这两类单元时原样返回同一个数组，纯历史轮次因此不会因为这一层而换引用、
 * 让轮次的 memo 边界失效。
 */
export function orderTurnCells(cells: TranscriptCell[]): TranscriptCell[] {
  const hasSummary = cells.some((cell) => cell.role === 'summary');
  if (!hasSummary && !cells.some(appendedWhileRunning)) return cells;
  const lastWorked = lastWorkedIndex(cells);
  const head: TranscriptCell[] = [];
  const summaries: TranscriptCell[] = [];
  const tail: TranscriptCell[] = [];
  cells.forEach((cell, index) => {
    if (cell.role === 'summary') {
      summaries.push(cell);
      return;
    }
    (waitsAtTail(cells, index, lastWorked) ? tail : head).push(cell);
  });
  if (tail.length > 1) tail.sort((left, right) => steerSequence(left) - steerSequence(right));
  return [...head, ...summaries, ...tail];
}
