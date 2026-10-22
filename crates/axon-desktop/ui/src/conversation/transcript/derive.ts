/*
 * 派生读取与纯工具函数。
 *
 * 这一层不持有任何状态：全部输入来自订阅流归约出的会话状态或工具载荷，
 * 输出是可直接渲染的值。后端没有事实来源的东西一律不在这里凭空空造。
 */

import { formatDurationMillisCompact } from '../format';
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
 * 派生：仍在排队等待安全边界插入的消息条数。
 *
 * 判据是本地乐观标识：durable 的用户消息事件按正文认领这一格后标识会换成
 * 服务端标识，计数自然归零，因此这里不需要第二份队列状态。已判定未送达的
 * 不再计入：它等的那次注入永远不会来了。
 */
export function pendingSteerCount(turns: TurnState[]): number {
  const position = [...turns]
    .map((turn, index) => ({ turn, index }))
    .reverse()
    .find(({ turn }) => turn.status === 'pending' || turn.status === 'streaming');
  if (position === undefined) return 0;
  return position.turn.cells.filter(
    (cell) =>
      cell.role === 'user' &&
      cell.id.startsWith(LOCAL_STEER_PREFIX) &&
      cell.undelivered !== true,
  ).length;
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
