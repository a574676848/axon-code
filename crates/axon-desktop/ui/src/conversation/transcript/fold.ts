/*
 * 轮次折叠：把一整条 Interaction 压成一行摘要，并在压缩的同一刻释放正文。
 *
 * 大轮次的正文（工具输出、推理、diff）是界面内存的主要来源：一条会话里几十段
 * 几万字符的输出，就足以在渲染时把进程撑爆。折叠把 `cells` 换成单个折叠单元，
 * 工具输出与推理正文随即失去引用、被回收；展开时按标识重新回放该 Interaction，
 * 把算好的单元整段换回来。
 *
 * 判据只看体量，不看新旧：体量小的一轮折起来只会让人多按一次，没有收益。
 */

import type { TranscriptCell, TurnState } from './state';

/** 折叠的体量阈值（字符数）。 */
export const FOLD_TEXT_THRESHOLD = 4_000;

/** 折叠的单元数阈值：短而多的一轮同样会按行放大节点数。 */
export const FOLD_CELL_THRESHOLD = 40;

/** 摘要里最多列几个动作短语，其余并进「等」。 */
const SUMMARY_PHRASE_LIMIT = 3;

/**
 * 保留展开的最大轮次数。
 *
 * 与 TUI 的 cell history GC 同一口径：历史按轮次淘汰，只保留最近这些轮**展开**，
 * 更早的一律压成一行摘要。体量判据管的是「一轮有多重」，这个窗口管的是「留几轮」，
 * 两者叠加后界面内存随会话变长不再无限增长。
 */
export const MATERIALIZED_TURN_WINDOW = 8;

/** 工具类别到摘要短语的映射；认不出来的一律并进兜底短语。 */
const TOOL_PHRASES: Record<string, string> = {
  read_file: '已读取文件',
  write_file: '修改了文件',
  edit: '修改了文件',
  search: '已搜索代码',
  glob: '已匹配文件',
  shell_command: '已执行命令',
  write_stdin: '已操作终端',
  invoke_skill: '已调用技能',
  memory_search: '已检索记忆',
  memory_write: '已写入记忆',
  update_plan: '已更新计划',
  job_output: '已读取作业输出',
  job_kill: '已终止作业',
  delegate_to_fast: '已委托子代理',
  spawn_agent: '已创建子代理',
  send_message: '已发送消息',
  list_agents: '已查看子代理',
  ask_user_question: '已向用户提问',
  notes_write_file: '已写入便签',
  notes_append_to_file: '已写入便签',
  notes_read_file: '已读取便签',
};

/** 认不出来的工具统一并成这一句。 */
const TOOL_PHRASE_FALLBACK = '已调用工具';

/** 单个工具名到摘要短语。 */
export function toolPhrase(toolName: string): string {
  const bare = toolName.includes('.')
    ? toolName.slice(toolName.lastIndexOf('.') + 1)
    : toolName;
  return TOOL_PHRASES[bare] ?? TOOL_PHRASE_FALLBACK;
}

/** 单元的文字体量；不含正文的单元按 0 计。 */
function cellWeight(cell: TranscriptCell): number {
  switch (cell.role) {
    case 'user':
    case 'assistant':
    case 'reasoning':
    case 'failure':
      return cell.text.length;
    case 'tool':
      return cell.arguments.length + cell.output.length + cell.summary.length;
    case 'fileChange':
      return cell.diff.length;
    case 'plan':
      return cell.steps.reduce((total, step) => total + step.step.length, 0);
    case 'history':
    case 'notice':
      return cell.summary.length;
    default:
      return 0;
  }
}

/** 一轮的大致文字体量。 */
export function turnTextWeight(turn: TurnState): number {
  let weight = 0;
  for (const cell of turn.cells) weight += cellWeight(cell);
  return weight;
}

/**
 * 这一轮是否该折成一行。
 *
 * 运行中的轮次永不折叠：正看着它长出来；用户显式展开过的同样不折，
 * 否则刚点开就被收回去，等于点不动。
 */
export function shouldFold(turn: TurnState): boolean {
  if (!isFoldable(turn)) return false;
  return (
    turn.cells.length >= FOLD_CELL_THRESHOLD ||
    turnTextWeight(turn) >= FOLD_TEXT_THRESHOLD
  );
}

/**
 * 这一轮是否已经超出保留窗口，该按轮次淘汰。
 *
 * 只看位置，不看体量：论体量小、论数量多的会话同样会把内存堆起来。最新一轮永不
 * 淘汰，用户显式展开过的那一轮也不淘汰（那是他刚刚用过的，属于最近使用）。
 */
export function shouldEvictFromWindow(
  turn: TurnState,
  position: number,
  total: number,
): boolean {
  if (position >= total - 1) return false;
  if (!isFoldable(turn)) return false;
  return position < total - MATERIALIZED_TURN_WINDOW;
}

/** 折叠的前置条件：已收口、尚未折叠、且不是用户要求保持展开的。 */
function isFoldable(turn: TurnState): boolean {
  if (turn.folded || turn.keepExpanded) return false;
  return turn.status !== 'streaming' && turn.status !== 'pending';
}

/** 折叠时保留的用户那一句；取首行并截断。 */
export function foldedPrompt(turn: TurnState): string {
  for (const cell of turn.cells) {
    if (cell.role !== 'user') continue;
    const line = cell.text.split('\n').find((part) => part.trim().length > 0);
    if (line !== undefined) return line.trim();
  }
  return '';
}

/** 折叠摘要：动作短语按首次出现顺序去重，最多列三个，其余并成「等」。 */
export function foldedSummary(turn: TurnState): string {
  const phrases: string[] = [];
  for (const cell of turn.cells) {
    if (cell.role !== 'tool') continue;
    const phrase = toolPhrase(cell.toolName);
    if (!phrases.includes(phrase)) phrases.push(phrase);
  }
  if (phrases.length === 0) {
    // 没有工具动作的一轮（通常是长回复）：如实说明，不留空行。
    return turn.cells.some((cell) => cell.role === 'reasoning')
      ? '已生成长回复与推理'
      : '已生成长回复';
  }
  const head = phrases.slice(0, SUMMARY_PHRASE_LIMIT).join('，');
  return phrases.length > SUMMARY_PHRASE_LIMIT ? `${head}等` : head;
}

/**
 * 折叠一轮：丢弃正文单元，只留一行摘要。
 *
 * 这一步就是「释放」本身：返回的新轮次不再引用任何工具输出与推理正文。
 */
export function foldTurn(turn: TurnState, summary: string): TurnState {
  if (turn.folded) return turn;
  const cell: TranscriptCell = {
    id: `${turn.id}:folded`,
    role: 'folded',
    // 回放出来的轮次以 Interaction 标识为轮次标识；本地乐观轮次用 id 兜底。
    interactionId: turn.interactionId ?? turn.id,
    prompt: foldedPrompt(turn),
    summary,
    atMs: turn.startedAtMs,
  };
  return {
    ...turn,
    folded: true,
    summary,
    cells: [cell],
    index: { [cell.id]: 0 },
  };
}

/** 展开一轮：换回重新算出来的单元，并记下「用户要它保持展开」。 */
export function replaceTurnCells(turn: TurnState, cells: TranscriptCell[]): TurnState {
  const index: Record<string, number> = {};
  cells.forEach((cell, position) => {
    index[cell.id] = position;
  });
  return { ...turn, folded: false, keepExpanded: true, cells, index };
}
