/*
 * 正文里的网页链接：识别、草稿占位与提交展开。
 *
 * 与 TUI 的附件占位同一套语义：草稿里只放一个短占位符，提交时展开成原文，
 * 模型拿到的仍是完整地址。占位符因此同时承担三件事：正文里的位置、来源
 * （站点名）与撤销粒度（整段粘贴算一次编辑）。
 *
 * 只认 http 与 https：后端打开外部地址时也只放行这两个协议，
 * 因此这里不会造出一个点了必定失败的入口。
 */

/** 草稿里的一个链接占位。 */
export interface LinkToken {
  /** 草稿里的占位符，例如 `[toutiao.com]`。 */
  placeholder: string;
  /** 提交时还原的原始地址。 */
  url: string;
}

/** 正文里命中的一段地址。 */
export interface UrlMatch {
  url: string;
  start: number;
  end: number;
}

/** 切开正文后的一段：`url` 非空表示这一段是地址。 */
export interface UrlRun {
  text: string;
  url?: string;
}

/** 一次粘贴的占位结果。 */
export interface TokenizedPaste {
  /** 这一段粘贴文本换成占位符之后的样子。 */
  text: string;
  /** 连同既有占位一起的完整清单，既有占位在前。 */
  tokens: LinkToken[];
}

/**
 * 地址本体：到空白、引号、尖括号、中文与全角标点为止。
 *
 * 中文与全角标点必须一起截断：中文句子里的地址后面常常不加空格，
 * 不截断会把后面的整段中文吞进链接。
 */
const URL_PATTERN = /\bhttps?:\/\/[^\s<>"'`\u3000-\u303f\u4e00-\u9fff\uff00-\uffef]+/gi;

/**
 * 地址结尾的句读：这些字符不属于地址本身。
 *
 * 问号不在此列：`?` 是查询串的开头，地址末尾带一个空查询也仍然是这段地址，
 * 剥掉就等于改了用户粘进来的 URL。中文标点由 URL_PATTERN 直接截断，不靠这里。
 */
const TRAILING_PUNCTUATION = /[.,;:!…]+$/u;

/** 成对收尾符号：只有右括号多于左括号时才剥掉，`Foo_(bar)` 因此保持完整。 */
const CLOSING_PAIRS: Readonly<Record<string, string | undefined>> = {
  ')': '(',
  ']': '[',
  '}': '{',
};

/** 站点名的字符上限：占位符只是一个标记，畸形主机名不该把输入框撑开。 */
const MAX_HOST_CHARS = 40;

/** 统计一个字符出现的次数。 */
function countOf(text: string, char: string): number {
  let count = 0;
  for (const value of text) {
    if (value === char) count += 1;
  }
  return count;
}

/**
 * 剥掉地址末尾的句读与不成对的右括号。
 *
 * 循环而不是一次替换：`https://x.com/a).` 要先剥句点再剥括号，
 * 顺序反过来会把括号留在地址里。
 */
function trimmedUrl(raw: string): string {
  let value = raw;
  for (;;) {
    const withoutPunctuation = value.replace(TRAILING_PUNCTUATION, '');
    if (withoutPunctuation !== value) {
      value = withoutPunctuation;
      continue;
    }
    const last = value.slice(-1);
    const opener = CLOSING_PAIRS[last];
    if (opener === undefined) return value;
    if (countOf(value, last) > countOf(value, opener)) {
      value = value.slice(0, -1);
      continue;
    }
    return value;
  }
}

/** 正文里全部 http 与 https 地址，按出现顺序返回。 */
export function findUrls(text: string): UrlMatch[] {
  const matches: UrlMatch[] = [];
  URL_PATTERN.lastIndex = 0;
  let match = URL_PATTERN.exec(text);
  while (match !== null) {
    const url = trimmedUrl(match[0]);
    // 只剩协议头的不算地址：`https://` 后面没有任何可打开的内容。
    if (url.slice(url.indexOf('://') + 3).length > 0) {
      matches.push({ url, start: match.index, end: match.index + url.length });
    }
    match = URL_PATTERN.exec(text);
  }
  return matches;
}

/**
 * 地址的站点名：去掉 `www.` 与 IPv6 的方括号，保留端口。
 *
 * 端口保留是有用的信息：`127.0.0.1:3143` 与 `127.0.0.1:8080` 是两个入口。
 */
export function linkHost(url: string): string {
  try {
    const parsed = new URL(url);
    return parsed.host.replace(/^www\./i, '').replace(/[[\]]/g, '').slice(0, MAX_HOST_CHARS);
  } catch {
    return '链接';
  }
}

/** 卡片与芯片上的短地址：站点加路径，去掉查询串、井号与结尾的斜杠。 */
export function compactUrlLabel(url: string): string {
  try {
    const parsed = new URL(url);
    const host = parsed.host.replace(/^www\./i, '');
    return `${host}${parsed.pathname.replace(/\/+$/u, '')}`;
  } catch {
    return url;
  }
}

/**
 * 分配一个占位符。
 *
 * 基准就是站点名，因此草稿里一眼能看出这条链接指向哪里；同站的不同地址、
 * 以及草稿里恰好写着同样文字的段落都会把基准挤走，改用带序号的后缀。
 */
function allocatePlaceholder(url: string, taken: string, used: ReadonlySet<string>): string {
  const host = linkHost(url);
  const base = `[${host}]`;
  if (!taken.includes(base) && !used.has(base)) return base;
  for (let suffix = 2; ; suffix += 1) {
    const candidate = `[${host} #${suffix}]`;
    if (!taken.includes(candidate) && !used.has(candidate)) return candidate;
  }
}

/**
 * 把一段粘贴文本里的地址换成占位符。
 *
 * @param pasted 剪贴板里的这一段文本。
 * @param draft 当前完整草稿：已有的同样文字会抢走占位符，因此要一起比对。
 * @param known 已经登记过的占位：同一地址复用，已用掉的文字不重复分配。
 * @returns 没有任何地址时返回 null，调用方按原样粘贴。
 */
export function tokenizePastedText(
  pasted: string,
  draft: string,
  known: readonly LinkToken[],
): TokenizedPaste | null {
  const matches = findUrls(pasted);
  if (matches.length === 0) return null;
  const tokens = [...known];
  const byUrl = new Map(known.map((token) => [token.url, token.placeholder]));
  const used = new Set(known.map((token) => token.placeholder));
  // 冲突比对同时看草稿与这一段粘贴文本：两边已有的文字都不能再当占位符。
  const taken = `${draft}${pasted}`;
  let output = '';
  let cursor = 0;
  for (const match of matches) {
    output += pasted.slice(cursor, match.start);
    let placeholder = byUrl.get(match.url);
    if (placeholder === undefined) {
      placeholder = allocatePlaceholder(match.url, taken, used);
      byUrl.set(match.url, placeholder);
      used.add(placeholder);
      tokens.push({ placeholder, url: match.url });
    }
    output += placeholder;
    cursor = match.end;
  }
  output += pasted.slice(cursor);
  return { text: output, tokens };
}

/** 仍然出现在草稿里的占位，按正文位置排序。 */
export function liveLinks(text: string, tokens: readonly LinkToken[]): LinkToken[] {
  const live: { token: LinkToken; at: number }[] = [];
  for (const token of tokens) {
    const at = text.indexOf(token.placeholder);
    if (at >= 0) live.push({ token, at });
  }
  live.sort((left, right) => left.at - right.at);
  return live.map((entry) => entry.token);
}

/** 草稿里一段命中的占位。 */
interface PlaceholderMatch {
  start: number;
  end: number;
  url: string;
}

/** 扫出全部占位命中，同一个占位出现多次时逐次列出。 */
function placeholderMatches(text: string, tokens: readonly LinkToken[]): PlaceholderMatch[] {
  const matches: PlaceholderMatch[] = [];
  for (const token of tokens) {
    let from = 0;
    for (;;) {
      const at = text.indexOf(token.placeholder, from);
      if (at < 0) break;
      matches.push({ start: at, end: at + token.placeholder.length, url: token.url });
      from = at + token.placeholder.length;
    }
  }
  matches.sort((left, right) => left.start - right.start);
  return matches;
}

/** 按命中区间切开正文；命中那一段带上原始地址。 */
function sliceRuns(text: string, matches: readonly { start: number; end: number; url: string }[]): UrlRun[] {
  if (matches.length === 0) return [{ text }];
  const runs: UrlRun[] = [];
  let cursor = 0;
  for (const match of matches) {
    if (match.start > cursor) runs.push({ text: text.slice(cursor, match.start) });
    runs.push({ text: text.slice(match.start, match.end), url: match.url });
    cursor = match.end;
  }
  if (cursor < text.length) runs.push({ text: text.slice(cursor) });
  return runs;
}

/**
 * 把草稿里的占位展开回原始地址。
 *
 * 单趟按位置拼接，而不是逐个占位做字符串替换：地址本身可能含有方括号
 * （IPv6 字面量），替换会把它当成下一轮的占位。
 */
export function expandLinks(text: string, tokens: readonly LinkToken[]): string {
  const matches = placeholderMatches(text, tokens);
  if (matches.length === 0) return text;
  let output = '';
  let cursor = 0;
  for (const match of matches) {
    output += text.slice(cursor, match.start) + match.url;
    cursor = match.end;
  }
  return output + text.slice(cursor);
}

/** 输入框底衬用：按草稿里的占位切开正文。 */
export function splitPlaceholderRuns(text: string, tokens: readonly LinkToken[]): UrlRun[] {
  return sliceRuns(text, placeholderMatches(text, tokens));
}

/** 消息气泡用：按原始地址切开正文。 */
export function splitUrlRuns(text: string): UrlRun[] {
  return sliceRuns(text, findUrls(text));
}
