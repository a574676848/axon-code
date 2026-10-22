/*
 * 行内标记的扫描。
 *
 * 与渲染分开：这一层只认文本与位置，产出的记号由渲染层拼成元素。分开的直接原因是
 * 渲染层是 JSX（测试加载器不解析 JSX），而这里正是出过事故的地方，必须能被单测守住。
 *
 * **每次扫描各建一个扫描器**：带 `g` 标志的正则把 `lastIndex` 记在正则对象自己身上，
 * 而强调类记号要递归扫描它的内部文本；共用同一个正则对象时，递归会把 `lastIndex`
 * 改到内层字符串的位置，外层回来继续扫描就从一个更早的位置重新命中同一个记号，
 * 光标不前进、记号无限增长（真机表现为渲染进程内存被吃光、界面卡死）。
 */

/** 行内记号的类别。 */
export type InlineTokenKind = 'image' | 'code' | 'strong' | 'strike' | 'em' | 'link';

/** 一个行内记号。 */
export interface InlineToken {
  /** 记号类别。 */
  kind: InlineTokenKind;
  /** 记号原文（含定界符）。 */
  text: string;
  /** 记号在源文本里的起点。 */
  index: number;
  /** 强调类记号的内部文本；其余类别为空串。 */
  inner: string;
}

/** 行内标记的模式；刻意不带 `g` 标志，扫描器由每次扫描各自新建。 */
export const INLINE_PATTERN =
  /(!\[[^\]]*\]\([^)]+\))|(`[^`]+`)|(\*\*[^*]+\*\*)|(~~[^~]+~~)|(\*[^*]+\*)|(\[[^\]]+\]\([^)]+\))/;

/** 把一个记号分类。 */
export function classifyInlineToken(token: string): InlineTokenKind | null {
  if (token.startsWith('![')) return 'image';
  if (token.startsWith('`')) return 'code';
  if (token.startsWith('**')) return 'strong';
  if (token.startsWith('~~')) return 'strike';
  if (token.startsWith('*')) return 'em';
  if (token.startsWith('[')) return 'link';
  return null;
}

/** 记号的内部文本（去掉定界符）；不适用时为空串。 */
export function inlineTokenInner(token: string, kind: InlineTokenKind): string {
  switch (kind) {
    case 'strong':
      return token.slice(2, -2);
    case 'strike':
      return token.slice(2, -2);
    case 'em':
      return token.slice(1, -1);
    case 'code':
      return token.slice(1, -1);
    default:
      return '';
  }
}

/**
 * 扫描一行文本里的行内记号，按出现顺序返回。
 *
 * 两段记号之间的普通文本由调用方按 `index` 与上一段的结束位置自行补上。
 */
export function scanInline(text: string): InlineToken[] {
  const tokens: InlineToken[] = [];
  const scanner = new RegExp(INLINE_PATTERN.source, 'g');
  let cursor = 0;
  let match = scanner.exec(text);
  while (match !== null) {
    // 光标必须单调前进：任何情况下都不允许原地打转。
    if (match.index < cursor) {
      scanner.lastIndex = cursor;
      match = scanner.exec(text);
      continue;
    }
    const raw = match[0];
    const kind = classifyInlineToken(raw);
    if (kind === null) {
      scanner.lastIndex = match.index + Math.max(1, raw.length);
      match = scanner.exec(text);
      continue;
    }
    tokens.push({
      kind,
      text: raw,
      index: match.index,
      inner: inlineTokenInner(raw, kind),
    });
    cursor = match.index + raw.length;
    scanner.lastIndex = Math.max(scanner.lastIndex, cursor);
    match = scanner.exec(text);
  }
  return tokens;
}
