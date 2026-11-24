/*
 * 朗读分段。
 *
 * 本地合成按次加载模型，整段长文一次送进合成既慢又容易超时；而朗读本身是
 * 一串句子，天然可以按句切分。这里把口播稿切成若干短段：
 *
 * 1. 先按句末标点与换行切；
 * 2. 仍然过长的句子再按逗号、顿号切；
 * 3. 最后按长度硬切，保证每一段都短于上限。
 *
 * 切分只影响「怎么送进合成」，不改读出来的内容顺序。
 */

/** 单段上限：中文约一百多字，本地合成通常在数秒内完成。 */
export const SEGMENT_MAX_CHARS = 120;

/** 句末标点：切在这里最自然。 */
const SENTENCE_END = /[。！？!?；;\n]+/;

/** 句中标点：长句退而求其次的切点。 */
const CLAUSE_END = /[，,、：:]+/;

/** 按标点切分并保留标点在前一段末尾。 */
function splitKeepingDelimiters(text: string, pattern: RegExp): string[] {
  const parts: string[] = [];
  let start = 0;
  const global = new RegExp(pattern.source, 'g');
  for (const match of text.matchAll(global)) {
    const end = (match.index ?? 0) + match[0].length;
    parts.push(text.slice(start, end));
    start = end;
  }
  if (start < text.length) parts.push(text.slice(start));
  return parts;
}

/** 按长度硬切，尽量落在断点附近。 */
function hardSplit(text: string, limit: number): string[] {
  const parts: string[] = [];
  let rest = text;
  while (rest.length > limit) {
    parts.push(rest.slice(0, limit));
    rest = rest.slice(limit);
  }
  if (rest.length > 0) parts.push(rest);
  return parts;
}

/** 把口播稿切成朗读段；空稿返回空数组。 */
export function splitSpeechSegments(
  script: string,
  limit: number = SEGMENT_MAX_CHARS,
): string[] {
  const trimmed = script.trim();
  if (trimmed.length === 0) return [];
  const maxChars = limit > 0 ? limit : SEGMENT_MAX_CHARS;

  const segments: string[] = [];
  for (const sentence of splitKeepingDelimiters(trimmed, SENTENCE_END)) {
    const piece = sentence.trim();
    if (piece.length === 0) continue;
    if (piece.length <= maxChars) {
      segments.push(piece);
      continue;
    }
    // 过长：先按句中标点拆，再对仍然超长的片段硬切。
    let buffer = '';
    for (const clause of splitKeepingDelimiters(piece, CLAUSE_END)) {
      if ((buffer + clause).length > maxChars && buffer.length > 0) {
        segments.push(buffer);
        buffer = '';
      }
      if (clause.length > maxChars) {
        for (const slice of hardSplit(clause, maxChars)) segments.push(slice);
        continue;
      }
      buffer += clause;
    }
    if (buffer.trim().length > 0) segments.push(buffer);
  }

  // 合并过短的相邻段：一句一个请求太碎，合成开销全在启动上。
  const merged: string[] = [];
  for (const segment of segments) {
    const previous = merged[merged.length - 1];
    if (previous !== undefined && (previous + segment).length <= maxChars) {
      merged[merged.length - 1] = previous + segment;
    } else {
      merged.push(segment);
    }
  }
  return merged;
}
