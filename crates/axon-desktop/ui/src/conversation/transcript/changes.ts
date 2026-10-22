/*
 * 一轮之内的文件改动事实。
 *
 * 事实源是订阅流上的 `fileChange` wire 事件：后端在每次文件工具落盘后宣告一条，
 * 携带路径、变更类型与正文（update 是 unified diff，add 与 delete 是整份内容）。
 * 归并口径与 TUI 的 diff 单元一致：`partId` 是稳定身份，重复宣告同一身份时原位
 * 覆盖而不是累加，否则后端的流式预览与收尾快照会把同一处改动算两遍。
 *
 * 无内容的宣告不进入事实：搜索类工具会为每个命中宣告一条空 update，收尾快照也会
 * 把已宣告过的改动以空 diff 再发一次，两者都不是改动。
 */

import type { WireFileChange } from '../wire';

/** 变更类型，与后端 `FileChange` 的三个变体一一对应。 */
export type FileChangeKind = 'add' | 'delete' | 'update';

/** 一条改动事实。 */
export interface FileChangePart {
  /** 事实身份；同一 partId 重复宣告时原位覆盖。 */
  partId: string;
  /** 工作区内的相对路径。 */
  path: string;
  /** 变更类型。 */
  kind: FileChangeKind;
  /** update 是 unified diff 原文；add 与 delete 是整份内容。 */
  text: string;
  /** 重命名后的目标路径；非重命名为 null。 */
  movePath: string | null;
  /** 宣告这次改动的工具调用单元标识；无来源工具时为 null。 */
  toolPartId: string | null;
}

/** 一轮的改动事实：partId 到事实的映射，插入顺序即宣告顺序。 */
export interface TurnChanges {
  parts: Record<string, FileChangePart>;
}

/** 空改动。 */
export const EMPTY_TURN_CHANGES: TurnChanges = { parts: {} };

/** 一个被改动文件的聚合；同一路径的多条事实按宣告顺序合成一段对比。 */
export interface ChangedFile {
  /** 工作区内的相对路径。 */
  path: string;
  /** 最后一次事实的变更类型，决定标题上写「新增」「删除」还是「修改」。 */
  kind: FileChangeKind;
  /** 重命名后的目标路径；非重命名为 null。 */
  movePath: string | null;
  added: number;
  removed: number;
  /** 按宣告顺序排列的片段，供对比视图逐段渲染。 */
  segments: { partId: string; kind: FileChangeKind; text: string }[];
}

/** 一段文本的行数；末尾换行不额外算一行。 */
export function countLines(text: string): number {
  if (text.length === 0) return 0;
  return text.replace(/\n$/, '').split('\n').length;
}

/**
 * 数 unified diff 里带指定标记的行。
 *
 * 只有第一个 hunk 之前的 `---` 与 `+++` 是文件头；hunk 之内以三连字符开头的行是
 * 「内容本身以 `--` 开头」的删除行（实测真实改动里就有），按内容计入。
 * 判定口径必须与对比视图的解析一致，否则卡片上的行数会与它下面渲染出来的行对不上。
 */
export function countDiffLines(text: string, marker: '+' | '-'): number {
  let counted = 0;
  let insideHunk = false;
  for (const line of text.split('\n')) {
    if (line.startsWith('@@')) {
      insideHunk = true;
      continue;
    }
    if (!insideHunk && (line.startsWith('--- ') || line.startsWith('+++ '))) continue;
    // 无末尾换行标记不是内容行。
    if (line.startsWith('\\')) continue;
    if (line.startsWith(marker)) counted += 1;
  }
  return counted;
}

/** 一条事实的新增与删除行数。 */
export function lineCountsOf(kind: FileChangeKind, text: string): { added: number; removed: number } {
  if (kind === 'add') return { added: countLines(text), removed: 0 };
  if (kind === 'delete') return { added: 0, removed: countLines(text) };
  return { added: countDiffLines(text, '+'), removed: countDiffLines(text, '-') };
}

/** 取一条 wire 事实的正文：update 用 unified diff，其余用整份内容。 */
function textOf(change: WireFileChange): string {
  return change.change.type === 'update' ? change.change.unified_diff : change.change.content;
}

/** 取一条 wire 事实的重命名目标；只有 update 带这个字段。 */
function movePathOf(change: WireFileChange): string | null {
  return change.change.type === 'update' ? (change.change.move_path ?? null) : null;
}

/**
 * 归并一条改动事实。
 *
 * 只丢弃「update 且正文为空」的宣告：它要么是搜索类工具对命中文件的空宣告，
 * 要么是收尾快照对已宣告改动的重复；新建与删除即使内容为空也是真实事实。
 */
export function accumulateFileChange(changes: TurnChanges, change: WireFileChange): TurnChanges {
  const text = textOf(change);
  const kind = change.change.type;
  if (kind === 'update' && text.trim().length === 0) return changes;
  const part: FileChangePart = {
    partId: change.partId,
    path: change.path,
    kind,
    text,
    movePath: movePathOf(change),
    toolPartId: change.toolPartId ?? null,
  };
  return { parts: { ...changes.parts, [part.partId]: part } };
}

/** 事实条数。 */
export function changePartCount(changes: TurnChanges): number {
  return Object.keys(changes.parts).length;
}

/**
 * 派生：按路径聚合的改动文件列表。
 *
 * 顺序取该路径第一次被宣告的位置，因此列表读起来与这一轮实际动手的顺序一致。
 */
export function changedFilesOf(changes: TurnChanges): ChangedFile[] {
  const byPath = new Map<string, ChangedFile>();
  for (const part of Object.values(changes.parts)) {
    const counts = lineCountsOf(part.kind, part.text);
    const existing = byPath.get(part.path);
    if (existing === undefined) {
      byPath.set(part.path, {
        path: part.path,
        kind: part.kind,
        movePath: part.movePath,
        added: counts.added,
        removed: counts.removed,
        segments: [{ partId: part.partId, kind: part.kind, text: part.text }],
      });
      continue;
    }
    existing.kind = part.kind;
    existing.movePath = part.movePath ?? existing.movePath;
    existing.added += counts.added;
    existing.removed += counts.removed;
    existing.segments.push({ partId: part.partId, kind: part.kind, text: part.text });
  }
  return [...byPath.values()];
}

/**
 * 派生：某个工具调用名下的改动汇总。
 *
 * 工具行上的改动摘要由事实派生而不是逐条累加：同一处改动被后端重复宣告时，
 * 派生结果天然只有一份，而累加会把行数算两遍。
 */
export function toolChangeSummary(
  changes: TurnChanges,
  toolPartId: string,
): { path: string; added: number; removed: number; diff: string } | null {
  let path: string | null = null;
  let added = 0;
  let removed = 0;
  const segments: string[] = [];
  for (const part of Object.values(changes.parts)) {
    if (part.toolPartId !== toolPartId) continue;
    const counts = lineCountsOf(part.kind, part.text);
    path = part.path;
    added += counts.added;
    removed += counts.removed;
    segments.push(part.text);
  }
  if (path === null) return null;
  return { path, added, removed, diff: segments.join('\n') };
}

/** 派生：改动文件列表的合计行数。 */
export function changeTotals(files: ChangedFile[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const file of files) {
    added += file.added;
    removed += file.removed;
  }
  return { added, removed };
}

/** 变更类型的中文名。 */
export const CHANGE_KIND_LABEL: Record<FileChangeKind, string> = {
  add: '新增',
  delete: '删除',
  update: '修改',
};
