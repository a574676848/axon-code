/*
 * 正文里的技能指令切分：把「叫某个已安装技能」的片段挑出来，供上色使用。
 *
 * 只认精确命中词表的 `/name`：斜杠落在正文开头或空白之后，名字由词字符与连字符组成，
 * 且它后面必须是空白或正文结束。因此 `/skills/x` 是路径、`/plan.md` 是文件名、
 * `看/看` 是随手写的斜杠，三者都不点亮；后端认的也是同一份名单，界面与提交口径一致。
 */

/** 正文里的一段：`skill` 为真表示这一段是技能指令。 */
export interface SkillTextSegment {
  text: string;
  skill: boolean;
}

/** 词字符加连字符即技能名，与已安装技能名的实际形态一致。 */
const TOKEN_PATTERN = /(^|\s)\/([\w-]+)(?=\s|$)/g;

/**
 * 按词表切分正文。
 *
 * @param text - 正文。
 * @param names - 已安装技能名。
 * @returns 按原文顺序切好的若干段；没有命中时就是整段原文。
 */
export function splitSkillTokens(text: string, names: readonly string[]): SkillTextSegment[] {
  if (text.length === 0) return [];
  if (names.length === 0) return [{ text, skill: false }];
  const lexicon = new Set(names);
  const segments: SkillTextSegment[] = [];
  let cursor = 0;
  TOKEN_PATTERN.lastIndex = 0;
  let match = TOKEN_PATTERN.exec(text);
  while (match !== null) {
    const name = match[2] ?? '';
    if (lexicon.has(name)) {
      // 前导空白不参与高亮：它是两段之间的分隔，不是指令本身。
      const start = match.index + (match[1]?.length ?? 0);
      const end = start + name.length + 1;
      if (start > cursor) segments.push({ text: text.slice(cursor, start), skill: false });
      segments.push({ text: text.slice(start, end), skill: true });
      cursor = end;
    }
    match = TOKEN_PATTERN.exec(text);
  }
  if (cursor < text.length) segments.push({ text: text.slice(cursor), skill: false });
  return segments;
}
