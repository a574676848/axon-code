/*
 * 输入坞的 `@` 文件引用：候选扫描规则与提交投影。
 *
 * 与 TUI 的路径补全同口径：一次只扫描请求的那一层目录，命中按
 * 精确 > 前缀 > 子串 排序，目录额外加权并可在选中后继续下钻。
 *
 * 引用在正文里以 `@<相对路径>` 呈现；提交时镜像 TUI 的
 * `append_tui_mentions`，把仍然看得见的引用收进一段结构化输入块。
 * 两者的差别只有一个：TUI 用 cwd，桌面端固定在当前工作区根目录，
 * 因为文件索引接口本身就是工作区作用域的（越界路径会被后端拒绝）。
 */

import type { FileEntryItem } from '../types';
import { stripQuestionReceiptBlock } from './questions';

/** 候选项条数上限；GUI 一屏能扫完的条数比终端少。 */
export const REFERENCE_CANDIDATE_LIMIT = 8;

/** 候选项的排序权重；与 TUI 的路径补全同序。 */
const EXACT_MATCH_SCORE = 1_000;
const PREFIX_MATCH_SCORE = 750;
const SUBSTRING_MATCH_SCORE = 500;
/** 目录额外加权：同名时目录排在文件前面，便于继续下钻。 */
const DIRECTORY_SCORE_BONUS = 8;

/** 结构化输入块的标题；与 TUI 提交投影逐字一致。 */
const STRUCTURED_INPUT_TITLE = '[结构化输入]';
/** 文件引用的展示前缀；与 TUI 提交投影逐字一致。 */
const REFERENCE_LINE_PREFIX = '已引用文件：';
/** 入站附件的展示前缀：只有附件、没有正文时，它承担这条消息的正文。 */
const ATTACHMENT_LINE_PREFIX = '已附加文件：';

/** 正文里一个活跃的 `@` 引用 token。 */
export interface ReferenceToken {
  /** `@` 在正文中的下标。 */
  start: number;
  /** 光标位置，也就是 token 的结束下标。 */
  end: number;
  /** token 文本，不含 `@`。 */
  query: string;
  /** query 里最后一段斜杠之前的部分；空表示工作区根目录。 */
  directory: string;
  /** 用来做前缀匹配的最后一段。 */
  prefix: string;
}

/**
 * 从光标位置向前找活跃的 `@` 引用。
 *
 * 判据与终端一致：`@` 之前必须是行首或空白（避免把邮箱地址当成引用），
 * 且它与光标之间不能出现空白，否则这一次引用已经输入结束。
 */
export function activeReferenceToken(text: string, caret: number): ReferenceToken | null {
  const end = Math.max(0, Math.min(caret, text.length));
  const start = text.lastIndexOf('@', end - 1);
  if (start < 0) return null;
  const before = start === 0 ? '' : text[start - 1];
  if (before !== '' && !/\s/.test(before)) return null;
  const query = text.slice(start + 1, end);
  if (/[\s@]/.test(query)) return null;
  return { start, end, query, ...splitDirectoryAndPrefix(query) };
}

/** 把 token 拆成目录与最后一段前缀。 */
export function splitDirectoryAndPrefix(query: string): { directory: string; prefix: string } {
  const normalized = query.replace(/\\/g, '/');
  const cut = normalized.lastIndexOf('/');
  if (cut < 0) return { directory: '', prefix: normalized };
  return { directory: normalized.slice(0, cut + 1), prefix: normalized.slice(cut + 1) };
}

/** 相对工作区根目录的目录名；根目录用空串表示（与索引接口一致）。 */
export function directoryArgument(directory: string): string {
  return directory.replace(/^\/+/, '').replace(/\/+$/, '');
}

/** 按前缀挑出候选，目录优先、精确 > 前缀 > 子串；与 TUI 同序。 */
export function matchReferenceCandidates(
  entries: readonly FileEntryItem[],
  prefix: string,
  limit = REFERENCE_CANDIDATE_LIMIT,
): FileEntryItem[] {
  const scored: { entry: FileEntryItem; score: number }[] = [];
  for (const entry of entries) {
    const score = matchScore(prefix, entry.name, entry.kind === 'dir');
    if (score === null) continue;
    scored.push({ entry, score });
  }
  scored.sort(
    (left, right) =>
      right.score - left.score || left.entry.name.localeCompare(right.entry.name),
  );
  return scored.slice(0, limit).map((item) => item.entry);
}

function matchScore(prefix: string, name: string, isDirectory: boolean): number | null {
  const query = prefix.toLowerCase();
  const candidate = name.toLowerCase();
  const base =
    query.length === 0
      ? PREFIX_MATCH_SCORE
      : candidate === query
        ? EXACT_MATCH_SCORE
        : candidate.startsWith(query)
          ? PREFIX_MATCH_SCORE
          : candidate.includes(query)
            ? SUBSTRING_MATCH_SCORE
            : null;
  if (base === null) return null;
  return base + (isDirectory ? DIRECTORY_SCORE_BONUS : 0);
}

/**
 * 选中候选项后写回正文的文本（不含 token 之前的目录）。
 *
 * 目录以 `/` 结尾，光标停在那里可以继续下钻；文件后面补一个空格，
 * 这一次引用随之结束，与终端里「选中即完成当前 token」的语义一致。
 */
export function referenceInsertion(entry: FileEntryItem): string {
  return entry.kind === 'dir' ? `${entry.name}/` : `${entry.name} `;
}

/** 引用在正文里的可见写法。 */
export function referenceLabel(relativePath: string): string {
  return `@${relativePath}`;
}

/**
 * 提交时把引用与附件投影成结构化输入块。
 *
 * 引用部分与 TUI 的 `append_tui_mentions` 逐字一致（`[结构化输入]` 段 + 每条
 * `已引用文件：<相对路径>`）；正文里已经看不到的引用不提交，重复引用只提交一次。
 *
 * 附件行（`已附加文件：<绝对路径>`）**只要有附件就写**，不区分正文是否为空：
 * 附件在链路中途被丢弃时（运行中追加、历史回放）模型至少还能拿到真实路径，
 * 而不是只看到一句「看下这张图」。只有附件、没有正文时这段块同时承担正文，
 * 否则接口会以「正文为空」拒收。
 *
 * 它属于结构化输入，回放时由 [`visibleUserText`] 剥掉，用户看到的气泡仍然只有
 * 自己敲的那一段与附件卡。
 */
export function structuredInputBlock(
  text: string,
  references: readonly string[],
  attachments: readonly string[] = [],
): string {
  const visible = references.filter(
    (path, index) => references.indexOf(path) === index && text.includes(referenceLabel(path)),
  );
  const uniqueAttachments = attachments.filter(
    (path, index) => attachments.indexOf(path) === index,
  );
  const lines = [
    ...visible.map((path) => `${REFERENCE_LINE_PREFIX}${path}`),
    ...uniqueAttachments.map((path) => `${ATTACHMENT_LINE_PREFIX}${path}`),
  ];
  if (lines.length === 0) return '';
  // 正文非空时这段块只是补充事实，因此带前导空行接在正文之后；
  // 正文为空时它承担整条正文：提交链路会裁掉前置空白，标题必须落在第 0 位，
  // 回放时才剥得干净。
  const head =
    text.trim().length > 0
      ? `\n\n${STRUCTURED_INPUT_TITLE}\n`
      : `${STRUCTURED_INPUT_TITLE}\n`;
  return `${head}${lines.join('\n')}`;
}

/**
 * 一条用户消息的可见正文。
 *
 * 消息内容里可能带着两类结构化块：提交时附上的结构化输入块（桌面端与 TUI 都会
 * 这么发），以及作答后附上的问答回执块（后端把答案写成一条用户消息）。回放时
 * 两者都不该当正文展示，这里各剥掉结尾那一段，正文里出现的同名文字不受影响。
 */
export function visibleUserText(content: string): string {
  return stripStructuredInputBlock(stripQuestionReceiptBlock(content));
}

/** 剥掉结尾的结构化输入块；正文里恰好写了同名标题时不误剥。 */
function stripStructuredInputBlock(content: string): string {
  const marker = `\n\n${STRUCTURED_INPUT_TITLE}\n`;
  const cut = content.lastIndexOf(marker);
  if (cut >= 0) {
    const tail = content.slice(cut + marker.length);
    if (isStructuredInputLines(tail)) return content.slice(0, cut);
  }
  // 只有附件、没有正文时整条正文就是这个块：提交链路会裁掉前置空白，
  // 标题因此可能落在第 0 位。
  const head = `${STRUCTURED_INPUT_TITLE}\n`;
  if (!content.startsWith(head)) return content;
  return isStructuredInputLines(content.slice(head.length)) ? '' : content;
}

/**
 * 块尾部的判据：整段都是引用行或附加行才算结构化输入块。
 *
 * 正文里恰好写了同名标题、后面跟着自己的话时不算块，原样保留。
 */
function isStructuredInputLines(tail: string): boolean {
  return (
    tail.length > 0 &&
    tail
      .split('\n')
      .every(
        (line) => line.startsWith(REFERENCE_LINE_PREFIX) || line.startsWith(ATTACHMENT_LINE_PREFIX),
      )
  );
}

/**
 * 把多段结构化输入块合成一段。
 *
 * 中断转投会把几条消息并成一次提交，而每一段都自带标题：直接拼接会出现多个
 * `[结构化输入]` 标题，回放时只剥得掉最后一段，前面那几段会当正文露出来。
 * 因此这里只保留一份标题，块尾的引用行与附件行按原顺序去重合并。
 */
export function mergeStructuredInput(blocks: readonly string[], text: string): string {
  const lines: string[] = [];
  for (const block of blocks) {
    const head = `${STRUCTURED_INPUT_TITLE}\n`;
    const body = block.startsWith(head)
      ? block.slice(head.length)
      : block.slice(block.lastIndexOf(`\n${STRUCTURED_INPUT_TITLE}\n`) + head.length + 1);
    for (const line of body.split('\n')) {
      if (line.length === 0 || lines.includes(line)) continue;
      if (!line.startsWith(REFERENCE_LINE_PREFIX) && !line.startsWith(ATTACHMENT_LINE_PREFIX)) continue;
      lines.push(line);
    }
  }
  if (lines.length === 0) return '';
  const head =
    text.trim().length > 0 ? `\n\n${STRUCTURED_INPUT_TITLE}\n` : `${STRUCTURED_INPUT_TITLE}\n`;
  return `${head}${lines.join('\n')}`;
}
