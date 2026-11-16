/*
 * 整轮折叠：把一轮的过程从正文里撤下，只留一根过程控件。
 *
 * 折叠不搬动任何一句话。过程单元（思考、工具、改动、历史、计划）整段撤下并释放正文，
 * 用户气泡与助手回复原位保留：控件插在开场输入之后，因此「叙述 A → 工具 → 叙述 B」
 * 折叠后读作「叙述 A → 叙述 B」，工具只由控件表达「存在」，不表达位置。
 *
 * 只折已经收口、且过程之后不再收到输入的轮次。运行中、中断、失败，以及过程行之后又
 * 出现用户消息的轮次一律保持展开：把输入压到过程之前会让这一轮读成另一种顺序。
 *
 * 重新打开会话时更早的轮次只有索引（用户那一句、工具名串、最终交付正文），因此按索引
 * 立起同一形态：用户气泡 + 控件 + 最终答案。中间叙述要等点开回放才补齐，界面不声称它有。
 */

import { visibleUserText } from '../references';
import { attachmentsFromPaths } from './derive';
import type { TranscriptCell, TurnState } from './state';
import { emptyTurn } from './turns';

/** 过程单元：折叠时整段撤下，正文随即失去引用。 */
const PROCESS_ROLES: ReadonlySet<TranscriptCell['role']> = new Set([
  'reasoning',
  'tool',
  'fileChange',
  'history',
  'plan',
]);

/** 内容单元：折叠后原位保留。收尾分割线不在其中，它随过程一起折掉。 */
const CONTENT_ROLES: ReadonlySet<TranscriptCell['role']> = new Set([
  'user',
  'assistant',
  'notice',
  'failure',
  'question',
]);

/** 摘要里最多列几个动作短语，其余并进「等」。 */
const SUMMARY_PHRASE_LIMIT = 3;

/** 工具类别到摘要短语的映射；认不出来的一律并进兜底短语。 */
const TOOL_PHRASES: Record<string, string> = {
  read_file: '已读取文件',
  write_file: '修改了文件',
  edit: '修改了文件',
  search: '已搜索代码',
  glob: '已匹配文件',
  shell_command: '执行了命令',
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
function toolPhrase(toolName: string): string {
  const bare = toolName.includes('.')
    ? toolName.slice(toolName.lastIndexOf('.') + 1)
    : toolName;
  return TOOL_PHRASES[bare] ?? TOOL_PHRASE_FALLBACK;
}

/** 这一格是不是过程。 */
function isProcessCell(cell: TranscriptCell): boolean {
  return PROCESS_ROLES.has(cell.role);
}

/** 这一格是不是要原位保留的内容。 */
function isContentCell(cell: TranscriptCell): boolean {
  return CONTENT_ROLES.has(cell.role);
}

/** 开场输入的条数：从第一条起连续的用户消息，控件插在它们之后。 */
function leadingUserCount(cells: readonly TranscriptCell[]): number {
  let count = 0;
  while (count < cells.length && cells[count].role === 'user') count += 1;
  return count;
}

/**
 * 过程之后的用户输入：控件只能落在开场输入之后，因此这类轮次不折叠。
 *
 * 判据是「第一条过程单元之后还有用户消息」，与它是否已被模型接住无关：位置本身就是
 * 读者要的先后，折叠会让它排到过程之前。
 */
function inputAfterProcess(turn: TurnState): boolean {
  let seenProcess = false;
  for (const cell of turn.cells) {
    if (isProcessCell(cell)) {
      seenProcess = true;
      continue;
    }
    if (seenProcess && cell.role === 'user') return true;
  }
  return false;
}

/** 这一轮有没有可折的过程。没有过程时折叠与展开完全一致，因此不做这件事。 */
function hasProcess(turn: TurnState): boolean {
  return turn.cells.some(isProcessCell);
}

/** 动作短语收口：最多列三个，其余并成「等」。 */
function phraseSummary(phrases: readonly string[]): string {
  const head = phrases.slice(0, SUMMARY_PHRASE_LIMIT).join('，');
  return phrases.length > SUMMARY_PHRASE_LIMIT ? `${head}等` : head;
}

/** 过程段的动作摘要：工具名优先，其次按过程类别给一句，全都没有时为空。 */
export function processSummary(cells: readonly TranscriptCell[]): string {
  const phrases: string[] = [];
  for (const cell of cells) {
    if (cell.role !== 'tool') continue;
    const phrase = toolPhrase(cell.toolName);
    if (!phrases.includes(phrase)) phrases.push(phrase);
  }
  if (phrases.length > 0) return phraseSummary(phrases);
  if (cells.some((cell) => cell.role === 'reasoning')) return '已思考';
  if (cells.some((cell) => cell.role === 'fileChange')) return '改动了文件';
  if (cells.some((cell) => cell.role === 'plan')) return '更新了计划';
  if (cells.some((cell) => cell.role === 'history')) return '压缩了上下文';
  return '';
}

/** 索引里的状态到界面短语。 */
function statusText(status: string): string {
  switch (status) {
    case 'interrupted':
      return '已中断';
    case 'rejected':
      return '未执行';
    default:
      return '已完成';
  }
}

/** 按单元顺序重建下标表。 */
function indexOf(cells: readonly TranscriptCell[]): Record<string, number> {
  const index: Record<string, number> = {};
  cells.forEach((cell, position) => {
    index[cell.id] = position;
  });
  return index;
}

/** 过程控件的标识：一轮一根，标识稳定不随折叠次数变化。 */
function controlId(turnId: string): string {
  return `${turnId}:control`;
}

/**
 * 这一轮是否该折。
 *
 * 位置先决：最新一轮永远展开（它是正在读的那一段）。其余条件都是「折了会读错」的
 * 情形，因此宁可让它继续展开。
 */
export function shouldFold(turn: TurnState, position: number, total: number): boolean {
  if (position >= total - 1) return false;
  if (turn.folded || turn.keepExpanded) return false;
  if (turn.status !== 'completed') return false;
  if (!hasProcess(turn)) return false;
  return !inputAfterProcess(turn);
}

/**
 * 折叠一轮：撤下全部过程单元，在开场输入之后插一根过程控件。
 *
 * 这一步就是「释放」本身：返回的新轮次不再引用任何工具输出与推理正文。内容单元的相对
 * 顺序完全不变，因此折叠前后读者看到的那几句话顺序一致。
 */
export function foldTurn(turn: TurnState): TurnState {
  if (turn.folded) return turn;
  const process = turn.cells.filter(isProcessCell);
  if (process.length === 0) return turn;
  const content = turn.cells.filter(isContentCell);
  const summary = processSummary(process);
  const control: TranscriptCell = {
    id: controlId(turn.id),
    role: 'folded',
    // 回放出来的轮次以 Interaction 标识为轮次标识；本地乐观轮次用 id 兜底。
    interactionId: turn.interactionId ?? turn.id,
    summary: summary.length > 0 ? summary : '过程已收起',
    elapsedMs: turn.elapsedMs,
    atMs: turn.startedAtMs,
  };
  const lead = leadingUserCount(content);
  const cells = [...content.slice(0, lead), control, ...content.slice(lead)];
  return {
    ...turn,
    folded: true,
    summary: control.summary,
    cells,
    index: indexOf(cells),
  };
}

/**
 * 展开一轮：换回重新算出来的单元，并记下「用户要它保持展开」。
 */
export function replaceTurnCells(turn: TurnState, cells: TranscriptCell[]): TurnState {
  return { ...turn, folded: false, keepExpanded: true, cells, index: indexOf(cells) };
}

/** 按索引立起的一轮：用户气泡 + 过程控件 + 最终答案。 */
export interface IndexedTurnEntry {
  interactionId: string;
  /** 入站正文。 */
  content?: string;
  /** 入站时落地的本地附件绝对路径。 */
  attachments?: readonly string[];
  receivedAtMs: number;
  /** 处理状态：received、processing、understood、rejected 或 interrupted。 */
  status: string;
  /** 该轮用过的工具名（逗号分隔、已去重）；老网关不返回时为空。 */
  toolSummary?: string | null;
  /** 本轮最终交付正文；老网关、非正常收口或正文已过保留期时为空。 */
  answerText?: string | null;
  /** 已收口轮次的墙钟用时（毫秒）。 */
  elapsedMs?: number | null;
}

/**
 * 索引轮：正文还没取，但这一轮该显示什么已经由索引决定。
 *
 * 形态与折叠出来的一轮完全一致（用户气泡 + 控件 + 最终答案），因此渲染与展开走的都是
 * 既有那条路径：点开控件时按 `interactionId` 回放该轮，再由 `replace-turn-cells` 换成
 * 完整正文。
 */
export function indexedTurn(entry: IndexedTurnEntry): TurnState {
  const base = emptyTurn(entry.interactionId, entry.receivedAtMs);
  const cells: TranscriptCell[] = [];
  // 索引进来的正文可能带着提交时的结构化输入块与问答回执块：两种都不该当气泡正文展示。
  const text = visibleUserText(entry.content ?? '');
  const attachments = attachmentsFromPaths([...(entry.attachments ?? [])]);
  if (text.trim().length > 0 || attachments.length > 0) {
    cells.push({
      id: `${entry.interactionId}:user`,
      role: 'user',
      text,
      atMs: entry.receivedAtMs,
      attachments,
    });
  }
  const summary = indexedSummary(entry);
  cells.push({
    id: controlId(entry.interactionId),
    role: 'folded',
    interactionId: entry.interactionId,
    summary,
    elapsedMs: entry.elapsedMs ?? null,
    atMs: entry.receivedAtMs,
  });
  const answer = (entry.answerText ?? '').trim();
  if (answer.length > 0 && settledStatus(entry.status) === 'completed') {
    cells.push({
      id: `${entry.interactionId}:answer`,
      role: 'assistant',
      text: answer,
      streaming: false,
      // 索引不含这一格的记录时间：宁可不显示时间，也不拿提问时间冒充回答时间。
      atMs: null,
    });
  }
  return {
    ...base,
    interactionId: entry.interactionId,
    status: settledStatus(entry.status),
    startedAtMs: entry.receivedAtMs,
    elapsedMs: entry.elapsedMs ?? null,
    folded: true,
    summary,
    cells,
    index: indexOf(cells),
  };
}

/**
 * 索引轮的控件标题。
 *
 * 有工具名就是那句动作摘要；没有工具名时给状态短语（已完成、已中断、未执行）。
 * 每一条已收口的索引轮都留这根控件：索引只说明这一轮怎么结束，说不清里面有没有过程，
 * 因此它同时是「点开加载全过程」的唯一入口。纯问答轮点开只会看到同样的正文，代价是
 * 一次用户主动发起的回放，而不是默认就替所有人回放一遍。
 */
function indexedSummary(entry: IndexedTurnEntry): string {
  const phrases: string[] = [];
  for (const name of (entry.toolSummary ?? '').split(',')) {
    const trimmed = name.trim();
    if (trimmed.length === 0) continue;
    const phrase = toolPhrase(trimmed);
    if (!phrases.includes(phrase)) phrases.push(phrase);
  }
  if (phrases.length > 0) return phraseSummary(phrases);
  return statusText(entry.status);
}

/** 索引里的处理状态到轮次终态：读不出来的按已收口处理。 */
function settledStatus(status: string): TurnState['status'] {
  switch (status) {
    case 'interrupted':
      return 'interrupted';
    case 'rejected':
      return 'rejected';
    default:
      return 'completed';
  }
}
