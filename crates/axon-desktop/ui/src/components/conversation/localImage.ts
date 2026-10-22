/*
 * 正文里的本地图片路径：判定、补全与工作区相对路径解析。
 *
 * 对话正文里的图片有两种写法，两种都要认：
 *   1) 标准 Markdown 图片语法 `![说明](路径)`；
 *   2) 工具回执里直接贴着路径的裸文本（截图路径、生成的图表路径）。
 *
 * 模型给出的路径未必干净：中文全角冒号、被引号或反引号包住、路径里带空格、
 * 反斜杠未转义。这一层只做纯字符串收敛，给出若干候选写法；
 * 真正判据只有后端有：后缀是已知图片类型、文件存在、体积在预览上限内。
 */

/** 可预览的图片后缀。与后端 `image_mime` 的清单保持一致。 */
const IMAGE_EXTENSIONS = new Set([
  'png',
  'jpg',
  'jpeg',
  'webp',
  'gif',
  'bmp',
  'ico',
  'svg',
  'tif',
  'tiff',
  'avif',
  'heic',
]);

/** 路径两侧可能粘上的包裹符；这些字符不属于路径本身。 */
const LEADING_WRAPPER = /^[`'"“”‘’<(\[{（【]+/;
const TRAILING_WRAPPER = /[`'"“”‘’>)\]}>）】]+$/;
/** 路径尾部可能粘上的句读；中英文都要收。 */
const TRAILING = /[.,;:!?、。；：！？…]+$/;

/** 枚举候选时，最多把后续几个词并进路径：够覆盖「带一个空格的目录名」。 */
const MAX_JOINED_WORDS = 2;

/** 后缀是否在可预览清单内。 */
export function isImagePath(value: string): boolean {
  const dot = value.lastIndexOf('.');
  if (dot <= 0 || dot === value.length - 1) return false;
  return IMAGE_EXTENSIONS.has(value.slice(dot + 1).toLowerCase());
}

/**
 * 去掉包裹符与尾部句读。
 *
 * 只剥两侧的包裹符与尾部句读，不碰路径内部的任何字符：路径里的空格与括号
 * 都是合法内容，动它们等于换一个文件。两侧分别剥、循环剥，是因为模型给出的
 * 写法常常是 `“E:/shots/a.png”。` 这种句读包着引号、引号再包着路径的嵌套。
 */
export function stripDecoration(value: string): string {
  let text = value.trim();
  for (;;) {
    const previous = text;
    text = text.replace(TRAILING, '').trim();
    text = text.replace(LEADING_WRAPPER, '').replace(TRAILING_WRAPPER, '').trim();
    if (text === previous) return text;
  }
}

/** 中文全角冒号写成盘符分隔符是常见笔误；只在这一处替换，不碰正文原文。 */
function normalizeDriveColon(value: string): string {
  return value.replace(/^([A-Za-z])：/, '$1:');
}

/** 一段词是否长得像路径：盘符、斜杠或反斜杠。 */
function looksPathLike(value: string): boolean {
  return /^(?:[A-Za-z]:[\\/]|\\\\|[\\/])/.test(value) || /[\\/]/.test(value);
}

/**
 * 正文兜底扫描：把一行里每一段可能的图片路径取出来。
 *
 * 裸文本没有语法边界，只能按空白切词，于是「目录名里带空格的绝对路径」会被切成
 * 好几段。判据是路径形状：一段以图片后缀结尾时它自己就是候选；一段只是长得像路径、
 * 本身还没有后缀时，就往后并词，直到并出图片后缀为止。
 *
 * 并到后缀就停，因此 `E:\我的 文档\a.png 这张图` 只会给出两个候选：
 * `E:\我的 文档\a.png` 与 `文档\a.png`（相对写法），不会把「这张图」也吞进路径。
 */
export function imagePathScanCandidates(line: string): { path: string; index: number }[] {
  const words: { text: string; index: number }[] = [];
  const pattern = /[^\s]+/g;
  let match = pattern.exec(line);
  while (match !== null) {
    words.push({ text: stripDecoration(match[0]), index: match.index });
    match = pattern.exec(line);
  }
  const out: { path: string; index: number }[] = [];
  const seen = new Set<string>();
  const push = (path: string, index: number) => {
    if (seen.has(path)) return;
    seen.add(path);
    out.push({ path: normalizeDriveColon(path), index });
  };
  words.forEach((word, position) => {
    // 既不是图片后缀、又不像路径的词（「见」「这张图」）连候选都不该产生。
    if (!isImagePath(word.text) && !looksPathLike(word.text)) return;
    // 已经带图片后缀的段自己就是一条完整候选。
    if (isImagePath(word.text)) {
      push(word.text, word.index);
      return;
    }
    for (let take = 1; take <= MAX_JOINED_WORDS; take += 1) {
      const next = words[position + take];
      if (next === undefined) break;
      const joined = `${word.text} ${next.text}`;
      if (!isImagePath(joined)) continue;
      push(joined, word.index);
      return;
    }
  });
  return out;
}

/**
 * 一个图片路径的全部候选写法，按「最可能」到「最宽松」排序。
 *
 * 绝大多数路径第一条就命中；只有路径被引号包住、粘着中文标点或用了全角冒号时
 * 才会用到后面的候选。候选之间不做存在性判断：存在与否只有后端知道。
 */
export function imagePathCandidates(raw: string): string[] {
  const out: string[] = [];
  const push = (value: string) => {
    const text = value.trim();
    if (text.length > 0 && !out.includes(text)) out.push(text);
  };
  const cleaned = stripDecoration(raw);
  push(cleaned);
  push(normalizeDriveColon(cleaned));
  return out;
}

/** Markdown 图片目标：解码 `%20` 这类转义，并去掉 `?query` 与 `#fragment` 后缀。 */
export function decodeImageDestination(destination: string): string {
  const withoutFragment = destination.split('#')[0] ?? destination;
  const withoutQuery = withoutFragment.split('?')[0] ?? withoutFragment;
  try {
    return decodeURIComponent(withoutQuery);
  } catch {
    // 转义不合法时用原文：宁可让后端报「不是图片」，也不要吞掉用户写的路径。
    return withoutQuery;
  }
}

/**
 * 图片节点的目标与说明文字。
 *
 * 三种写法都要认，它们是同一件事在 Markdown 里的三种合法拼法：
 * - `![alt](path)`；
 * - `![alt](<path>)`：路径含空格时的标准转义写法；
 * - `![alt](<path> "标题")`：带标题。
 *
 * 解析不出来返回 null，由调用方按普通文本处理。
 */
export function parseImageNode(token: string): { destination: string; alt: string } | null {
  const parts = /^!\[([^\]]*)\]\(\s*(?:<([^>]*)>|([^)\s]*))(?:\s+["'(].*)?\s*\)$/.exec(token);
  if (parts === null) return null;
  const destination = parts[2] ?? parts[3] ?? '';
  if (destination.trim().length === 0) return null;
  return { destination, alt: parts[1] ?? '' };
}

/** 盘符、UNC、根路径、家目录：明确的绝对路径形状。 */
const ABSOLUTE = /^(?:[A-Za-z]:[\\/]|\\\\|\/|~[\\/]?$|~[\\/])/;

/**
 * 相对路径落到工作区根：绝对路径与 `~/` 原样返回，相对路径拼到根下。
 *
 * 与输入坞 `@` 引用同口径（工作区作用域），因此正文里写 `assets/shot.png` 与
 * 输入坞里引用同一个文件指向同一张图。`~/` 交给后端展开：后端本来就有家目录
 * 展开口（`workspace_files::resolve_within`），前端再猜一个只会出现第二套事实。
 */
export function resolveWorkspacePath(workspaceRoot: string | null, path: string): string {
  const text = path.trim();
  if (text.length === 0) return text;
  if (ABSOLUTE.test(text)) return text;
  const root = workspaceRoot === null ? '' : workspaceRoot.trim().replace(/[\\/]+$/, '');
  if (root.length === 0) return text;
  const relative = text.replace(/\\/g, '/').replace(/^\.\//, '');
  return `${root}/${relative}`;
}

/**
 * 一个 Markdown 图片目标最终要交给后端读取的路径；`null` 表示不是本地路径
 * （HTTP(S) 图片由渲染层直接加载，不走本地读取通道）。
 */
export function localImagePath(workspaceRoot: string | null, destination: string): string | null {
  const decoded = decodeImageDestination(destination);
  if (decoded.trim().length === 0) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(decoded) && !/^[a-z]:[\\/]/i.test(decoded)) return null;
  return resolveWorkspacePath(workspaceRoot, decoded);
}
