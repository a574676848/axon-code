/*
 * unified diff 的呈现模型。
 *
 * 事实源是后端给的 diff 原文（update 是 unified diff，add 与 delete 是整份内容），
 * 这里只做「原文到可渲染行」的解析，不带任何渲染决策：文件头、hunk 头与无末尾换行
 * 标记都不算内容行，其余按首字符判新增、删除与上下文，并按 hunk 头的起始行号给两侧编号。
 */

import type { FileChangeKind } from './transcript';

/** 一行对比。 */
export interface DiffRow {
  kind: 'add' | 'delete' | 'context';
  /** 旧文件行号；新增行为 null。 */
  oldLine: number | null;
  /** 新文件行号；删除行为 null。 */
  newLine: number | null;
  text: string;
}

/** 一个 hunk；整份内容（新增与删除）没有 hunk 头。 */
export interface DiffHunk {
  header: string | null;
  rows: DiffRow[];
}

/** 解析结果。 */
export interface ParsedDiff {
  hunks: DiffHunk[];
  /** 行数超过上限被截断。 */
  truncated: boolean;
}

/** 单份对比最多渲染的行数；超出即截断并如实标注，避免一份巨型 diff 拖垮渲染。 */
export const MAX_DIFF_ROWS = 5000;

/** hunk 头：只要两侧的起始行号，行数由实际行数决定。 */
const HUNK_HEADER = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

/** 解析一份对比。 */
export function parseDiff(kind: FileChangeKind, text: string): ParsedDiff {
  const hunks: DiffHunk[] = [];
  if (text.length === 0) return { hunks, truncated: false };
  const lines = text.replace(/\n$/, '').split('\n');
  let truncated = false;
  let drawn = 0;
  let hunk: DiffHunk | null = null;
  let oldLine = 0;
  let newLine = 0;
  let insideHunk = false;

  /** 追加一行；超出上限时返回 false 让外层收手。 */
  const push = (row: DiffRow): boolean => {
    if (drawn >= MAX_DIFF_ROWS) {
      truncated = true;
      return false;
    }
    if (hunk === null) {
      const opened: DiffHunk = { header: null, rows: [] };
      hunks.push(opened);
      hunk = opened;
    }
    hunk.rows.push(row);
    drawn += 1;
    return true;
  };

  for (const line of lines) {
    if (kind === 'add') {
      newLine += 1;
      if (!push({ kind: 'add', oldLine: null, newLine, text: line })) break;
      continue;
    }
    if (kind === 'delete') {
      oldLine += 1;
      if (!push({ kind: 'delete', oldLine, newLine: null, text: line })) break;
      continue;
    }
    const header = HUNK_HEADER.exec(line);
    if (header !== null) {
      insideHunk = true;
      oldLine = Number(header[1]);
      newLine = Number(header[2]);
      const opened: DiffHunk = { header: line, rows: [] };
      hunks.push(opened);
      hunk = opened;
      continue;
    }
    // 文件头只出现在第一个 hunk 之前；hunk 之内以 `---` 开头的行是删掉的内容。
    if (!insideHunk && (line.startsWith('--- ') || line.startsWith('+++ '))) continue;
    // 无末尾换行标记不占任何一侧的行号。
    if (line.startsWith('\\')) continue;
    // hunk 头给的是两侧第一行的行号，因此先按当前值编号再前进。
    if (line.startsWith('+')) {
      const at = newLine;
      newLine += 1;
      if (!push({ kind: 'add', oldLine: null, newLine: at, text: line.slice(1) })) break;
      continue;
    }
    if (line.startsWith('-')) {
      const at = oldLine;
      oldLine += 1;
      if (!push({ kind: 'delete', oldLine: at, newLine: null, text: line.slice(1) })) break;
      continue;
    }
    const content = line.startsWith(' ') ? line.slice(1) : line;
    const oldAt = oldLine;
    const newAt = newLine;
    oldLine += 1;
    newLine += 1;
    if (!push({ kind: 'context', oldLine: oldAt, newLine: newAt, text: content })) break;
  }
  return { hunks, truncated };
}
