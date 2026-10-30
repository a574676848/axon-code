/*
 * 派生读取与纯工具函数。
 *
 * 这一层不持有任何状态：全部输入来自订阅流归约出的会话状态或工具载荷，
 * 输出是可直接渲染的值。后端没有事实来源的东西一律不在这里凭空空造。
 */

import { formatDurationMillisCompact } from '../format';
import { mergeStructuredInput } from '../references';
import { LOCAL_STEER_PREFIX, type AttachmentRef, type ConversationState, type JobRow, type TurnState } from './state';

/** 派生：后台作业行；只收 shell 与作业类工具，事实源是订阅流本身。 */
export function deriveJobs(state: ConversationState): JobRow[] {
  const jobs: JobRow[] = [];
  for (const turn of state.turns) {
    for (const cell of turn.cells) {
      if (cell.role !== 'tool') continue;
      if (!isJobTool(cell.toolKind, cell.toolName)) continue;
      jobs.push({
        id: cell.id,
        command: cell.command.length > 0 ? cell.command : cell.summary,
        status: cell.status,
        output: cell.output,
        durationMs: cell.durationMs,
        startedAtMs: cell.startedAtMs,
        exitCode: cell.exitCode,
        jobId: cell.jobId,
        fullOutputAvailable: cell.fullOutputAvailable,
      });
    }
  }
  return jobs;
}

/** 该工具是否属于后台作业类。 */
function isJobTool(toolKind: string, toolName: string): boolean {
  const kind = toolKind.toLowerCase();
  if (kind === 'shell' || kind === 'job') return true;
  const name = toolName.toLowerCase();
  return name.includes('shell') || name.includes('job_') || name.endsWith('.job');
}

/** 派生：紧凑时长文案。 */
export function compactDuration(durationMs: number | null): string {
  return durationMs === null ? '' : formatDurationMillisCompact(durationMs);
}

/**
 * 工作状态行文案。
 *
 * 与 TUI 的 composer dock 同口径：优先取 provider 实时阶段的完整文案，
 * 尚无阶段事件时回落到「{模型} 生成回复中」。中断提示由状态行自己拼装
 * （`按 ESC 中断`），不混进主文案里。
 */
export function workingStatusPrimary(activity: string | null, model: string): string {
  if (activity !== null && activity.trim().length > 0) return activity;
  const route = model.trim().length > 0 ? model.trim() : '主 Agent';
  return `${route} 生成回复中`;
}

/**
 * 中断时可交给下一轮的消息。
 *
 * 与 TUI 的 pending steer 同义：运行中追加、本轮还没给出答复。中断之后它必须按
 * 原样重发，因此正文、附件与结构化输入块都要带上。
 */
export interface CarriedPrompt {
  /** 正文里有那一格；交接时按标识把它从旧轮次摘走。 */
  cellId: string;
  text: string;
  attachments: AttachmentRef[];
  structuredInput: string;
}

/**
 * 追加序号：用户敲下这些消息的先后。
 *
 * 认领会改写标识与时间，注入顺序也不保证与提交顺序一致，唯一靠得住的是这一条
 * 自己带上的号。渲染与转投都按它排，历史帧里没有序号的老单元按 0 处理。
 */
export function steerSequence(cell: TurnState['cells'][number]): number {
  return cell.role === 'user' ? (cell.steerSeq ?? 0) : 0;
}

/**
 * 派生：仍在等安全边界注入的消息条数。
 *
 * 判据是本地排队标识：服务端认领（已注入本轮）之后标识换成 partId，计数随之归零。
 * 已交给下一轮的不再计入，它已经不在等本轮这个边界了。
 */
export function pendingSteerCount(turns: TurnState[]): number {
  return appendedCells(turns).filter(
    (cell) =>
      cell.role === 'user' &&
      cell.id.startsWith(LOCAL_STEER_PREFIX) &&
      cell.undelivered !== true &&
      cell.requeued !== true,
  ).length;
}

/**
 * 中断时可交给下一轮的消息，按用户先后排序。
 *
 * 判据是「运行中追加」这一身份，而不是本地排队标识：已注入本轮的面试被服务端认领
 * 了，可那一轮随后被中断、没给出答复，它们同样要接着跑，不能只转投还没被认领的。
 * 已经交出去的（requeued）不再出现，否则会被重复发送。
 */
export function requeueableSteers(turns: TurnState[]): CarriedPrompt[] {
  return appendedCells(turns)
    .filter((cell) => cell.role === 'user' && cell.requeued !== true)
    .sort((left, right) => steerSequence(left) - steerSequence(right))
    .map(toCarriedPrompt);
}

/**
 * 正在跑的那一轮里「运行中追加」的那些单元。
 *
 * 只看最后一条未收口的轮次：早先轮次里的追加消息都已经有结论了。
 */
function appendedCells(turns: TurnState[]): TurnState['cells'] {
  const running = [...turns]
    .reverse()
    .find((turn) => turn.status === 'pending' || turn.status === 'streaming');
  if (running === undefined) return [];
  return running.cells.filter((cell) => cell.role === 'user' && cell.steer === true);
}

/** 把排队单元投影成可重发的载荷。 */
function toCarriedPrompt(cell: TurnState['cells'][number]): CarriedPrompt {
  const user = cell as Extract<TurnState['cells'][number], { role: 'user' }>;
  return {
    cellId: user.id,
    text: user.text,
    attachments: user.attachments,
    structuredInput: user.structuredInput ?? '',
  };
}

/**
 * 把这一批待转投的消息并成一次提交。
 *
 * 后端一次提交只接受一条消息，而往运行中的轮次注入又只在安全边界发生：逐条发送
 * 既不能保证到达顺序，也会让后面几条落在模型已经开始思考之后（读起来就是回复
 * 夹在气泡中间）。并成一条提交则是一次原子的输入，顺序、位置与「回复在下面」
 * 同时成立。
 *
 * 正文按用户敲的先后用空行拼接，附件按路径去重，引用行与附件行合成一段结构化输入。
 */
export function mergeCarriedPrompts(
  prompts: readonly CarriedPrompt[],
): { text: string; attachments: AttachmentRef[]; structuredInput: string } | null {
  if (prompts.length === 0) return null;
  const text = prompts
    .map((prompt) => prompt.text.trim())
    .filter((item) => item.length > 0)
    .join('\n\n');
  const attachments: AttachmentRef[] = [];
  for (const prompt of prompts) {
    for (const attachment of prompt.attachments) {
      if (attachments.some((known) => known.path === attachment.path)) continue;
      attachments.push(attachment);
    }
  }
  const structuredInput = mergeStructuredInput(
    prompts.map((prompt) => prompt.structuredInput).filter((block) => block.length > 0),
    text,
  );
  return { text, attachments, structuredInput };
}

/** 由附件路径推导展示信息：文件名与是否可预览；历史事件只带路径，因此没有体量。 */
export function attachmentsFromPaths(paths: string[]): AttachmentRef[] {
  return paths.map((path) => {
    const name = path.split(/[\\/]/).pop() ?? path;
    return { path, name, image: IMAGE_EXTENSIONS.has(extensionOf(name)), bytes: null };
  });
}

/** 从工具参数摘要里取出命令行或文件路径。 */
export function previewCommand(argumentsSummary: string | undefined): string {
  if (argumentsSummary === undefined) return '';
  const trimmed = argumentsSummary.trim();
  if (!trimmed.startsWith('{')) return trimmed;
  try {
    const parsed = JSON.parse(trimmed) as Record<string, unknown>;
    const command = parsed.command;
    if (typeof command === 'string' && command.length > 0) return command;
    if (Array.isArray(command)) return command.map(String).join(' ');
    // 读写类工具的主文本是路径：它既是这一行最该看到的信息，
    // 也是右侧栏「点击打开文件」的入口。
    const path = parsed.path;
    if (typeof path === 'string' && path.length > 0) return path;
    return trimmed;
  } catch {
    return trimmed;
  }
}

/** 取一个已知为对象的嵌套字段。 */
export function nested(value: Record<string, unknown>, key: string): Record<string, unknown> | null {
  const inner = value[key];
  return typeof inner === 'object' && inner !== null ? (inner as Record<string, unknown>) : null;
}

/** 取非空字符串字段；空串与缺失同义。 */
export function stringField(value: Record<string, unknown>, key: string): string | null {
  const field = value[key];
  return typeof field === 'string' && field.length > 0 ? field : null;
}

/** 取数字字段；非数字返回 null。 */
export function numberField(value: Record<string, unknown>, key: string): number | null {
  const field = value[key];
  return typeof field === 'number' ? field : null;
}

/** 取布尔字段；只有显式 true 才算真。 */
export function booleanField(value: Record<string, unknown> | null, key: string): boolean {
  return value !== null && value[key] === true;
}

/** 可预览的图片扩展名。 */
const IMAGE_EXTENSIONS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg']);

/** 文件名的小写扩展名；无扩展名返回空串。 */
function extensionOf(name: string): string {
  const index = name.lastIndexOf('.');
  return index < 0 ? '' : name.slice(index + 1).toLowerCase();
}
