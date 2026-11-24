/*
 * 正文片段的切分：网页链接与技能指令。
 *
 * 两处正文共用这一份切分：用户消息气泡按原始地址认链接，输入框底衬按草稿里的
 * 占位认链接。顺序固定为「先认链接，再在剩下的文字里认技能」：地址里也有斜杠，
 * 反过来认会把 `https://x.com/plan` 拆出一个技能片段。
 */

import {
  splitPlaceholderRuns,
  splitUrlRuns,
  type LinkToken,
  type UrlRun,
} from './linkTokens';
import { splitSkillTokens } from './skillTokens';

/** 正文里的一段。 */
export interface TextSegment {
  kind: 'text' | 'skill' | 'link';
  text: string;
  /** `kind` 为 link 时的原始地址。 */
  url?: string;
}

/** 把普通文字按技能指令切开并追加到结果里。 */
function pushPlainSegments(
  out: TextSegment[],
  text: string,
  skillNames: readonly string[],
): void {
  for (const segment of splitSkillTokens(text, skillNames)) {
    out.push({ kind: segment.skill ? 'skill' : 'text', text: segment.text });
  }
}

/** 把切好的链接段与普通段收敛成一份片段序列。 */
function collectSegments(runs: readonly UrlRun[], skillNames: readonly string[]): TextSegment[] {
  const segments: TextSegment[] = [];
  for (const run of runs) {
    if (run.url === undefined) {
      pushPlainSegments(segments, run.text, skillNames);
      continue;
    }
    segments.push({ kind: 'link', text: run.text, url: run.url });
  }
  return segments;
}

/** 用户消息气泡：先认原始地址，剩下的文字再认技能指令。 */
export function splitMessageSegments(
  text: string,
  skillNames: readonly string[],
): TextSegment[] {
  return collectSegments(splitUrlRuns(text), skillNames);
}

/**
 * 输入框底衬：占位算链接，其余文字照旧认技能指令。
 *
 * 没有粘成占位的地址（例如逐字键入的）在这里保持普通文字：占位是粘贴这一动作
 * 的回执，输入框不替用户重写正在写的正文。
 */
export function splitComposerSegments(
  text: string,
  skillNames: readonly string[],
  tokens: readonly LinkToken[],
): TextSegment[] {
  return collectSegments(splitPlaceholderRuns(text, tokens), skillNames);
}
