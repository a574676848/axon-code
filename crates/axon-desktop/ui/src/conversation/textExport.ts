/*
 * 文本落到磁盘时用到的公共换算：大段粘贴的判定、导出文件命名、UTF-8 转 base64。
 *
 * 桌面端与 TUI 在这里的选择不同：TUI 在草稿里放一个占位符、提交时展开；
 * 桌面端有真实磁盘与附件通道，落成 Markdown 文档更合适，草稿不会被巨量文本撑爆，
 * 模型拿到的是可再次读取的绝对路径，用户也能在附件条上看见这个文件。
 *
 * 命名只在这一处定稿：下载正文与落地粘贴文本用的是同一套规则，因此不会出现
 * 同一份内容在两处得到两种文件名。规则按三平台取交集：
 * - 去掉 Windows 保留字符 `< > : " / \ | ? *` 与控制字符；
 * - 去掉结尾的点与空格（Windows 上这类名字非法）；
 * - 截断长度，避免中文标题把路径撑长；
 * - 一律保留可读的中文，不做拼音化。
 */

/**
 * 触发落盘的字符数阈值。
 *
 * 取值依据是「草稿框还读得下去」：输入坞最多向上展开 6 行，超过三千字符的文本
 * 留在框里只能靠滚，用户既看不全也改不动，不如直接变成可读取的文件。
 */
export const LARGE_TEXT_CHAR_THRESHOLD = 3000;

/** 标题最多保留的字符数。 */
export const TITLE_MAX_CHARS = 24;

/** 是否是应当落盘的大段文本。 */
export function isLargeText(text: string): boolean {
  return text.length > LARGE_TEXT_CHAR_THRESHOLD;
}

/** 两位补零。 */
function pad(value: number): string {
  return value.toString().padStart(2, '0');
}

/**
 * 紧凑时间戳，例如 20261007-1048。
 *
 * 用本地时间而不是 ISO 串：这些文件是给人认的，用户按自己看到的时刻找文件最直接；
 * 文件名里也不出现冒号与小数点，三平台都不会踩到非法字符。
 */
function compactStamp(now: Date): string {
  return (
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}`
  );
}

/** Windows 保留字符与控制字符；`/` 同时是 POSIX 的路径分隔符。 */
const ILLEGAL_IN_NAME = /[<>:"/\\|?*\u0000-\u001f]/g;

/** 结尾的标点不参与文件名：点与空格在 Windows 上非法，其余是排版噪声。 */
const TRAILING_NOISE = /[.\s、，。；：！？,;:!?-]+$/u;

/** 把一个候选标题收敛成文件名里的安全片段。 */
function sanitizeTitle(raw: string): string {
  const collapsed = raw.replace(/\s+/g, ' ').replace(ILLEGAL_IN_NAME, '-').trim();
  const capped =
    collapsed.length > TITLE_MAX_CHARS ? collapsed.slice(0, TITLE_MAX_CHARS) : collapsed;
  return capped.replace(TRAILING_NOISE, '').trim();
}

/** 行内标记只留可见文字：粗体、斜体、行内代码、链接与图片、行首标记都摘掉。 */
function stripInline(text: string): string {
  return text
    .replace(/^\s*>\s*/, '')
    .replace(/^\s*[-*+]\s+/, '')
    .replace(/^\s*\d+[.)]\s+/, '')
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/\*\*([^*]*)\*\*/g, '$1')
    .replace(/[*_~]/g, '')
    .replace(/<[^>]*>/g, '');
}

/**
 * 从正文里取一个能说明内容的标题。
 *
 * 取第一处真正的文字：优先标题行，其次正文首行；空白行、代码围栏与分割线跳过。
 * 拿不到可读文字时返回 null，由调用方退回按用途与时间命名。
 */
export function markdownTitle(source: string): string | null {
  // 围栏成对出现，其间的内容整段跳过：一段代码不是这份文档的标题。
  let inFence = false;
  for (const line of source.split('\n')) {
    const trimmed = line.trim();
    if (trimmed.startsWith('```') || trimmed.startsWith('~~~')) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;
    if (trimmed.length === 0) continue;
    if (/^[-*_]{3,}$/.test(trimmed)) continue;
    const heading = /^#{1,6}\s+(.*)$/.exec(trimmed);
    const title = sanitizeTitle(stripInline(heading === null ? trimmed : heading[1]));
    if (title.length > 0) return title;
  }
  return null;
}

/**
 * 导出文件名：正文标题加时间戳。
 *
 * 标题让用户在下载目录里一眼看出这份文档讲什么；时间戳保证同一标题多次导出不会互相覆盖。
 * 取不到标题时退回「用途 + 时间戳」，至少还能按类型与时间认出来。
 */
export function exportMarkdownName(
  kind: 'assistant' | 'pasted',
  source: string,
  now: Date,
): string {
  const stamp = compactStamp(now);
  const title = markdownTitle(source);
  if (title === null) return `${kind}-${stamp}.md`;
  // 粘贴件保留来源前缀：它是临时产物，用户需要知道这东西是怎么来的。
  return kind === 'pasted' ? `pasted-${title}-${stamp}.md` : `${title}-${stamp}.md`;
}

/** 一次性送进 fromCharCode 的字节数；再大就有展开参数过多导致栈溢出的风险。 */
const BASE64_CHUNK_BYTES = 0x8000;

/**
 * UTF-8 文本转 base64。
 *
 * 必须分块：`String.fromCharCode(...bytes)` 会把每个字节摊成一个实参，
 * 大段文本直接爆栈（这正是粘贴大文本这条路径最容易踩的坑）。
 */
export function base64FromText(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = '';
  for (let index = 0; index < bytes.length; index += BASE64_CHUNK_BYTES) {
    binary += String.fromCharCode(...bytes.subarray(index, index + BASE64_CHUNK_BYTES));
  }
  return btoa(binary);
}
