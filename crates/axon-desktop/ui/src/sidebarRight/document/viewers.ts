import { HIGHLIGHT_KEYS } from '../../components/conversation/codeHighlight';

/*
 * 文档查看页的渲染器注册表。
 *
 * 一份文件可能有多种看法（Markdown 文件既能渲染成文档，也能按源码看；SVG 既是图片
 * 也是源码），因此这里声明的是**候选表**而不是「一个后缀一个渲染器」的映射：
 * 按后缀命中的候选全部列出，由工具栏的「呈现方式」切换。
 *
 * 顺序就是优先级，也是菜单顺序，与既有工作台产品的注册顺序一致：
 * 纯文本（兜底）、Markdown、HTML、图片、PDF、Office、表格、代码。
 * 纯文本排在第一位不是因为它最具体，而是因为它是**兜底**：命中任何具体渲染器的后缀
 * 都会把它的候选追加到末尾，只有一个候选时工具栏不画切换入口。
 */

/** 渲染器的身份。 */
export type ViewerId =
  | 'plain'
  | 'markdown'
  | 'html'
  | 'image'
  | 'pdf'
  | 'office'
  | 'table'
  | 'code';

/** 渲染器怎么拿内容。 */
export type ViewerLoading =
  /** 按文本读（有 2 MiB 上限，超出如实标注截断）。 */
  | 'text'
  /** 按原始字节读（图片、PDF、HTML 必须拿到未解码的字节）。 */
  | 'bytes'
  /** 不读内容（例如 Office 在这台机器上没有可用的转换服务）。 */
  | 'none';

/** 一个渲染器的声明。 */
export interface ViewerDefinition {
  /** 身份，也是持久化在 tab 上的取值。 */
  id: ViewerId;
  /** 工具栏上的展示名。 */
  title: string;
  /** 认领的后缀（小写，不含点）。 */
  extensions: string[];
  /**
   * 该后缀属于二进制内容：命中时**不追加纯文本兜底**。
   *
   * 读不出可读文本的后缀如果还摆一个「纯文本」选项，点下去只会得到一屏乱码。
   */
  binary?: string[];
  /** 读取方式。 */
  loading: ViewerLoading;
  /** 是否支持自动换行。 */
  wrap: boolean;
}

/** 兜底渲染器：任何文本文件都能用这一种看。 */
const PLAIN: ViewerDefinition = {
  id: 'plain',
  title: '纯文本',
  extensions: [],
  loading: 'text',
  wrap: true,
};

/** 渲染器表；顺序即优先级与菜单顺序。 */
const REGISTERED: ViewerDefinition[] = [
  PLAIN,
  {
    id: 'markdown',
    title: 'Markdown',
    extensions: ['md', 'markdown'],
    loading: 'text',
    wrap: false,
  },
  {
    id: 'html',
    title: 'HTML',
    extensions: ['html', 'htm'],
    loading: 'bytes',
    wrap: false,
  },
  {
    id: 'image',
    title: '图片',
    extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'ico', 'svg'],
    // SVG 是文本，因此不列为二进制：它同时给出「代码」这个候选。
    binary: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'ico'],
    loading: 'bytes',
    wrap: false,
  },
  {
    id: 'pdf',
    title: 'PDF',
    extensions: ['pdf'],
    binary: ['pdf'],
    loading: 'bytes',
    wrap: false,
  },
  {
    id: 'office',
    title: 'Office 文档',
    extensions: ['doc', 'docx', 'ppt', 'pptx'],
    binary: ['doc', 'docx', 'ppt', 'pptx'],
    loading: 'none',
    wrap: false,
  },
  {
    id: 'table',
    title: '表格',
    // 只认分隔符文本：`.xlsx` / `.xls` 是需要表格引擎的压缩包，本工作台没有内置引擎，
    // 因此如实归入「暂时无法预览」，而不是给出一个打不开的入口。
    extensions: ['csv', 'tsv'],
    loading: 'text',
    wrap: false,
  },
  {
    id: 'code',
    title: '代码',
    // 能着色的后缀与高亮引擎共用一份来源，因此新增语言不必改这里。
    extensions: [...HIGHLIGHT_KEYS],
    loading: 'text',
    wrap: true,
  },
];

/**
 * 没有任何渲染器、也不是文本的后缀：直接给「暂时无法预览」的空态。
 *
 * 这份表决定的是「要不要发一次读取请求」：命中它时一次都不读，
 * 因此打开一个几百兆的视频不会先把字节读进内存再告诉用户看不了。
 */
const UNVIEWABLE_EXTENSIONS = new Set([
  'mp4', 'mov', 'avi', 'mkv', 'webm', 'mp3', 'wav', 'flac', 'ogg', 'm4a',
  'zip', 'gz', 'tar', '7z', 'rar', 'bz2', 'xz',
  'exe', 'dll', 'so', 'dylib', 'msi', 'apk', 'dmg', 'iso', 'bin',
  'ttf', 'otf', 'woff', 'woff2', 'eot',
  'psd', 'ai', 'sketch', 'fig',
  'xlsx', 'xls', 'db', 'sqlite', 'class', 'jar', 'wasm', 'pdf', // pdf 由渲染器认领，命中它时不会走到这里
]);

/** 取文件名的小写扩展名；没有扩展名时返回空串。 */
export function extensionOf(path: string): string {
  const name = path.split(/[\\/]/).pop() ?? path;
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

/**
 * 这份文件可以用哪几种方式看；返回空表表示「暂时无法预览」。
 *
 * 规则与既有工作台产品逐条对齐：
 *
 * 1. 命中的候选按注册顺序给出；
 * 2. 命中的后缀若被声明为二进制，就**不追加**纯文本兜底；
 * 3. 一个都没命中且后缀属于「没有渲染器也不是文本」时返回空表；
 * 4. 其余情况追加纯文本兜底，因此任何文本文件至少有这一种看法。
 */
export function viewersFor(path: string): ViewerDefinition[] {
  const extension = extensionOf(path);
  const matched = REGISTERED.filter((viewer) => viewer.extensions.includes(extension));
  if (matched.length > 0) {
    const binary = REGISTERED.some((viewer) => (viewer.binary ?? []).includes(extension));
    return binary ? matched : [...matched, PLAIN];
  }
  return UNVIEWABLE_EXTENSIONS.has(extension) ? [] : [PLAIN];
}

/** 渲染器表本身；空态文案要按后缀取类型图形时用得上。 */
export { REGISTERED as VIEWERS };
