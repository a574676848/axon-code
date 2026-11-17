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

/**
 * 工具类别到摘要短语的映射。
 *
 * 词表沿用工作台折叠组标题的既有契约：同一件事只该有一种说法，两边不必换算措辞。
 * 认不出来的工具统一并进 `tools`。
 */
const TOOL_CATEGORIES: Record<string, string> = {
  // 读取
  read: 'read',
  read_file: 'read',
  read_dir: 'read',
  read_image: 'readImage',
  view_image: 'readImage',
  read_mcp_resource: 'read',
  list_mcp_resources: 'read',
  notes_read_file: 'read',
  notes_list_files_by_prefix: 'read',
  history_list_items: 'read',
  history_list_windows: 'read',
  history_read_item: 'read',
  // 检索
  search: 'search',
  search_code: 'search',
  grep: 'search',
  glob: 'search',
  history_search_contents: 'search',
  notes_search_contents: 'search',
  // 写入与编辑
  write: 'write',
  write_file: 'write',
  notes_write_file: 'write',
  notes_append_to_file: 'write',
  edit: 'edit',
  apply_patch: 'edit',
  apply_diff: 'edit',
  hashline: 'edit',
  hashline_edit: 'edit',
  // 命令与代码
  shell: 'commands',
  shell_command: 'commands',
  exec_command: 'commands',
  write_stdin: 'commands',
  run_code: 'code',
  // 网页
  web_search: 'webSearch',
  web_fetch: 'webFetch',
  // 子代理与协作
  subagent: 'subagents',
  spawn_agent: 'subagents',
  delegate_to_fast: 'subagents',
  send_message: 'subagents',
  list_agents: 'subagents',
  // 计划与目标
  update_plan: 'plan',
  goal_create: 'plan',
  goal_update: 'plan',
  goal_start: 'plan',
  goal_cancel: 'plan',
  goal_get: 'plan',
  goal_list: 'plan',
  // Axon 特有的记忆与技能面
  memory_search: 'memory',
  memory_write: 'memory',
  invoke_skill: 'skill',
  // 提问
  ask_user_question: 'questions',
  request_user_input: 'questions',
};

/** 类别到已结束摘要短语（工作台既有措辞，逐字沿用）。 */
const CATEGORY_PHRASES: Record<string, string> = {
  read: '已读取文件',
  readImage: '已读取图片',
  search: '已搜索代码',
  write: '已写入文件',
  edit: '修改了文件',
  commands: '执行了命令',
  code: '运行了代码',
  webSearch: '已搜索网页',
  webFetch: '已访问网页',
  subagents: '已协调子智能体',
  plan: '更新了计划',
  questions: '向用户提出了问题',
  memory: '已使用记忆',
  skill: '已调用技能',
  tools: '已调用工具',
};

/** 没有任何工具类别时的摘要：这一轮只想了没动手。 */
const THINKING_PHRASE = '已完成分析';

/** 两个短语共用的前缀：都以它开头时，第二个前缀省略。 */
const SHARED_PREFIX = '已';

/** 单个工具名到类别。 */
function toolCategory(toolName: string): string {
  const bare = toolName.includes('.')
    ? toolName.slice(toolName.lastIndexOf('.') + 1)
    : toolName;
  return TOOL_CATEGORIES[bare] ?? 'tools';
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

/**
 * 这一轮有没有值得折叠的过程。
 *
 * 判据是「想过」或「动过手」：只有压缩标记、计划或文件改动的轮次不折，纯问答轮更不折。
 * 没有过程的轮次折起来只是一根没有信息的行，点开也看不到新内容。
 */
function hasProcess(turn: TurnState): boolean {
  return turn.cells.some(
    (cell) => cell.role === 'reasoning' || cell.role === 'tool',
  );
}

/**
 * 摘要收口：沿用工作台既有的组合规则。
 *
 * 一条直接用它自己；两条用「并」连，且都以「已」开头时第二条省略前缀（读作
 * `已读取文件并搜索代码`）；三条用顿号并列；超过三条只留前三条并补「等」。
 */
function phraseSummary(phrases: readonly string[]): string {
  if (phrases.length === 0) return '';
  const first = phrases[0];
  if (phrases.length === 1) return first;
  const second = phrases[1];
  if (phrases.length === 2) {
    const shared = first.startsWith(SHARED_PREFIX) && second.startsWith(SHARED_PREFIX);
    return `${first}并${shared ? second.slice(SHARED_PREFIX.length) : second}`;
  }
  const title = phrases.slice(0, SUMMARY_PHRASE_LIMIT).join('，');
  return phrases.length > SUMMARY_PHRASE_LIMIT ? `${title}等` : title;
}

/**
 * 过程段的摘要。
 *
 * 类别按出现次数排名、同次数按首次出现顺序；三端读同一份事实就该给同一句话，
 * 因此排序与措辞都照工作台折叠组标题的既有契约来。只有思考没有工具时给「已完成分析」，
 * 两者都没有时为空（调用方据此不立折叠行）。
 */
export function processSummary(cells: readonly TranscriptCell[]): string {
  const counts = new Map<string, number>();
  for (const cell of cells) {
    if (cell.role !== 'tool') continue;
    const category = toolCategory(cell.toolName);
    counts.set(category, (counts.get(category) ?? 0) + 1);
  }
  const thinking = cells.some((cell) => cell.role === 'reasoning');
  if (counts.size === 0) {
    if (thinking) return THINKING_PHRASE;
    if (cells.some((cell) => cell.role === 'fileChange')) return '修改了文件';
    if (cells.some((cell) => cell.role === 'plan')) return '更新了计划';
    if (cells.some((cell) => cell.role === 'history')) return '已完成分析';
    return '';
  }
  // Map 保留首次插入顺序：先按次数降序，同次数保持首次出现顺序。
  const ranked = [...counts.entries()].sort((left, right) => right[1] - left[1]);
  return phraseSummary(ranked.map(([category]) => CATEGORY_PHRASES[category] ?? '已调用工具'));
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
 * 展开一轮：换回重新算出来的单元，并在开头留一根可折叠的抬头。
 *
 * 展开态仍保留过程控件（箭头朝下）：用户能一口气看完全过程，也能再点一下把它收回去。
 * 收起不是「隐藏」而是「释放」：`collapseTurn` 会把过程单元整段丢掉，只留内容单元。
 */
export function replaceTurnCells(turn: TurnState, cells: TranscriptCell[]): TurnState {
  const summary = turn.summary.length > 0 ? turn.summary : processSummary(cells);
  const control: TranscriptCell = {
    id: controlId(turn.id),
    role: 'folded',
    interactionId: turn.interactionId ?? turn.id,
    summary: summary.length > 0 ? summary : '过程已收起',
    elapsedMs: turn.elapsedMs,
    atMs: turn.startedAtMs,
    expanded: true,
  };
  const content = cells.filter(isContentCell);
  const lead = leadingUserCount(content);
  const withControl = [control, ...cells];
  const insertAt = lead === 0 ? 0 : lead;
  const ordered = [...cells.slice(0, insertAt), control, ...cells.slice(insertAt)];
  void withControl;
  return {
    ...turn,
    folded: false,
    keepExpanded: true,
    expanded: true,
    summary: control.summary,
    cells: ordered,
    index: indexOf(ordered),
  };
}

/**
 * 收起一轮：撤下过程单元并释放正文，回到「一根抬头 + 内容」的形态。
 *
 * 这条路径与自动折叠共用同一份释放语义：收完之后轮次不再引用任何工具输出与推理正文。
 */
export function collapseTurn(turn: TurnState): TurnState {
  const process = turn.cells.filter(isProcessCell);
  const content = turn.cells.filter(isContentCell);
  const summary =
    turn.summary.length > 0
      ? turn.summary
      : processSummary(process).length > 0
        ? processSummary(process)
        : '过程已收起';
  const control: TranscriptCell = {
    id: controlId(turn.id),
    role: 'folded',
    interactionId: turn.interactionId ?? turn.id,
    summary,
    elapsedMs: turn.elapsedMs,
    atMs: turn.startedAtMs,
  };
  const lead = leadingUserCount(content);
  const cells = [...content.slice(0, lead), control, ...content.slice(lead)];
  return {
    ...turn,
    folded: true,
    keepExpanded: false,
    expanded: false,
    summary,
    cells,
    index: indexOf(cells),
  };
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
  /*
   * 只在这一轮确实动过手时立过程控件。
   *
   * 索引里没有「想过没有」这一项，可用的过程事实只有工具名串：它为空就说明索引说不出
   * 这一轮有过程，此时再挂一根控件，点开只会把同一段问答重放一遍，等于给纯问答轮加噪音。
   */
  const summary = indexedSummary(entry);
  if (summary.length > 0) {
    cells.push({
      id: controlId(entry.interactionId),
      role: 'folded',
      interactionId: entry.interactionId,
      summary,
      elapsedMs: entry.elapsedMs ?? null,
      atMs: entry.receivedAtMs,
    });
  }
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
 * 索引里可用的过程事实只有工具名串，因此标题就是按次数排名的类别摘要；串为空说明索引
 * 说不出这一轮有过程，此时返回空串，调用方据此不立控件（纯问答轮不点也看不到新内容）。
 */
function indexedSummary(entry: IndexedTurnEntry): string {
  const counts = new Map<string, number>();
  for (const name of (entry.toolSummary ?? '').split(',')) {
    const trimmed = name.trim();
    if (trimmed.length === 0) continue;
    const category = toolCategory(trimmed);
    counts.set(category, (counts.get(category) ?? 0) + 1);
  }
  const ranked = [...counts.entries()].sort((left, right) => right[1] - left[1]);
  return phraseSummary(ranked.map(([category]) => CATEGORY_PHRASES[category] ?? '已调用工具'));
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
