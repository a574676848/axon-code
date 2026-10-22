/**
 * 代码高亮内核：shiki。
 *
 * 采用 TextMate 语法定义，与主流编辑器共用同一套语言规则与主题变量：
 * 跨行字符串、模板嵌套、泛型、正则字面量等结构都能正确着色，
 * 不需要自己维护分词状态机，也不会出现「注释里的引号把后面全染成字符串」。
 * 引擎选纯 JS 正则实现，不依赖 WASM，WebView 里可以直接运行。
 *
 * 体积策略：本模块只被「确实要显示代码」的组件动态引入；每种语言的语法
 * 各自一个分包，打开什么文件才下载什么语法，首屏与不读代码的会话零开销。
 */

import type { CSSProperties } from 'react';
import type { HighlighterCore, LanguageRegistration, ThemedToken } from 'shiki/core';

/** 主题名；颜色全部交给 CSS 变量，换配色只改样式表。 */
const THEME_NAME = 'axon';

/** 变量前缀，与 tokens.css 中的声明一一对应。 */
const VARIABLE_PREFIX = '--shiki-';

/** 语言分包表；键即传给高亮引擎的语言名。 */
const LANGUAGE_LOADERS = {
  rust: () => import('@shikijs/langs/rust'),
  typescript: () => import('@shikijs/langs/typescript'),
  tsx: () => import('@shikijs/langs/tsx'),
  javascript: () => import('@shikijs/langs/javascript'),
  jsx: () => import('@shikijs/langs/jsx'),
  vue: () => import('@shikijs/langs/vue'),
  svelte: () => import('@shikijs/langs/svelte'),
  python: () => import('@shikijs/langs/python'),
  bash: () => import('@shikijs/langs/bash'),
  powershell: () => import('@shikijs/langs/powershell'),
  json: () => import('@shikijs/langs/json'),
  html: () => import('@shikijs/langs/html'),
  xml: () => import('@shikijs/langs/xml'),
  css: () => import('@shikijs/langs/css'),
  markdown: () => import('@shikijs/langs/markdown'),
  toml: () => import('@shikijs/langs/toml'),
  yaml: () => import('@shikijs/langs/yaml'),
  ini: () => import('@shikijs/langs/ini'),
  sql: () => import('@shikijs/langs/sql'),
  go: () => import('@shikijs/langs/go'),
  java: () => import('@shikijs/langs/java'),
  c: () => import('@shikijs/langs/c'),
  cpp: () => import('@shikijs/langs/cpp'),
  csharp: () => import('@shikijs/langs/csharp'),
  php: () => import('@shikijs/langs/php'),
  ruby: () => import('@shikijs/langs/ruby'),
  kotlin: () => import('@shikijs/langs/kotlin'),
  swift: () => import('@shikijs/langs/swift'),
  lua: () => import('@shikijs/langs/lua'),
  diff: () => import('@shikijs/langs/diff'),
  docker: () => import('@shikijs/langs/docker'),
  make: () => import('@shikijs/langs/make'),
  graphql: () => import('@shikijs/langs/graphql'),
  proto: () => import('@shikijs/langs/proto'),
};

/** 受支持的语言名。 */
export type HighlightLanguage = keyof typeof LANGUAGE_LOADERS;

/** 语言包模块形状。 */
type LanguageModule = { default: LanguageRegistration[] };

/**
 * 别名表：扩展名、围栏标记、常见简写都指向正式语言名。
 *
 * 高亮引擎自身也认别名，但别名依附于「已经加载的语言」，
 * 这里先归一化，才能在加载语言包之前就知道该拉哪个分包。
 */
const LANGUAGE_ALIASES: Record<string, HighlightLanguage> = {
  rs: 'rust',
  rust: 'rust',
  ts: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  typescript: 'typescript',
  tsx: 'tsx',
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  javascript: 'javascript',
  node: 'javascript',
  jsx: 'jsx',
  vue: 'vue',
  svelte: 'svelte',
  py: 'python',
  python: 'python',
  python3: 'python',
  sh: 'bash',
  bash: 'bash',
  shell: 'bash',
  zsh: 'bash',
  console: 'bash',
  ps1: 'powershell',
  psm1: 'powershell',
  powershell: 'powershell',
  json: 'json',
  jsonc: 'json',
  html: 'html',
  htm: 'html',
  xml: 'xml',
  svg: 'xml',
  plist: 'xml',
  xaml: 'xml',
  css: 'css',
  md: 'markdown',
  markdown: 'markdown',
  toml: 'toml',
  yaml: 'yaml',
  yml: 'yaml',
  ini: 'ini',
  conf: 'ini',
  cfg: 'ini',
  env: 'ini',
  properties: 'ini',
  sql: 'sql',
  go: 'go',
  java: 'java',
  c: 'c',
  h: 'c',
  cc: 'cpp',
  cpp: 'cpp',
  cxx: 'cpp',
  hpp: 'cpp',
  hh: 'cpp',
  cs: 'csharp',
  csharp: 'csharp',
  php: 'php',
  rb: 'ruby',
  ruby: 'ruby',
  kt: 'kotlin',
  kts: 'kotlin',
  kotlin: 'kotlin',
  swift: 'swift',
  lua: 'lua',
  diff: 'diff',
  patch: 'diff',
  dockerfile: 'docker',
  docker: 'docker',
  makefile: 'make',
  make: 'make',
  mk: 'make',
  graphql: 'graphql',
  gql: 'graphql',
  proto: 'proto',
  protobuf: 'proto',
};

/**
 * 别名表的键集合：既是「认得出这些写法」，也是「这些后缀的文件按代码呈现」。
 *
 * 查看页的代码渲染器直接用它当后缀表，因此新增一门语言只要在上面加别名，
 * 不必再去别处补一份后缀清单。
 */
export const HIGHLIGHT_KEYS: readonly string[] = Object.freeze(Object.keys(LANGUAGE_ALIASES));

/** 把任意写法归一化成受支持的语言名；不支持时返回 null。 */
export function resolveLanguage(name: string | null | undefined): HighlightLanguage | null {
  if (name === null || name === undefined) return null;
  const key = name.trim().toLowerCase();
  if (key.length === 0) return null;
  return LANGUAGE_ALIASES[key] ?? null;
}

/** 按文件路径推断语言：先看扩展名，再退回整名（Dockerfile、Makefile 这类无扩展名文件）。 */
export function languageOfPath(path: string): HighlightLanguage | null {
  const name = path.split(/[\\/]/).pop() ?? path;
  const dot = name.lastIndexOf('.');
  if (dot <= 0) return resolveLanguage(name);
  return resolveLanguage(name.slice(dot + 1)) ?? resolveLanguage(name);
}

/** 高亮器单例；首次真正需要着色时才创建。 */
let corePromise: Promise<HighlighterCore> | null = null;

/**
 * 单块着色的体量上限（字符数）。
 *
 * 着色结果按 token 生成节点，一段八万字符的工具输出能炸出几万个 `<span>`：
 * 主线程先卡死，随后渲染进程内存不足被系统回收（实测就是这样崩的）。
 * 超过上限时退回纯文本，代价只是没有颜色，内容一个字都不少。
 */
const MAX_HIGHLIGHT_CHARS = 20_000;

/** 单块着色的行数上限：短行密集的输出同样按行放大节点数。 */
const MAX_HIGHLIGHT_LINES = 2_000;

/** 这一段是否值得着色；超过任一上限都不着。 */
export function withinHighlightBudget(code: string): boolean {
  if (code.length > MAX_HIGHLIGHT_CHARS) return false;
  let lines = 1;
  for (let index = 0; index < code.length; index += 1) {
    if (code.charCodeAt(index) === 10) {
      lines += 1;
      if (lines > MAX_HIGHLIGHT_LINES) return false;
    }
  }
  return true;
}

/*
 * 引擎本体也用动态引入：本模块会被正文组件静态引用，若在这里直接引入
 * 高亮引擎，首屏就要为「当前会话里可能一行代码都没有」付这份体积。
 */
function core(): Promise<HighlighterCore> {
  corePromise ??= Promise.all([import('shiki/core'), import('shiki/engine/javascript')]).then(
    ([shiki, engine]) =>
      shiki.createHighlighterCore({
        themes: [
          shiki.createCssVariablesTheme({
            name: THEME_NAME,
            variablePrefix: VARIABLE_PREFIX,
            fontStyle: true,
          }),
        ],
        langs: [],
        engine: engine.createJavaScriptRegexEngine(),
      }),
  );
  return corePromise;
}

/** 已发起加载的语言；同一语言只加载一次。 */
const loading = new Map<HighlightLanguage, Promise<void>>();

function ensureLanguage(language: HighlightLanguage): Promise<void> {
  const existing = loading.get(language);
  if (existing !== undefined) return existing;
  const pending = LANGUAGE_LOADERS[language]()
    .then(async (module: LanguageModule) => {
      const instance = await core();
      await instance.loadLanguage(module.default);
    })
    .catch((error: unknown) => {
      // 失败的加载不留在缓存里，否则一次网络抖动会让这门语言永久不可用。
      loading.delete(language);
      throw error;
    });
  loading.set(language, pending);
  return pending;
}

/**
 * 高亮整段代码。
 *
 * 返回按行切分的记号；语言不受支持、体量超限或语法加载失败时返回 null，
 * 由调用方退回纯文本，绝不因为着色失败而让代码看不见。
 */
export async function highlightCode(
  code: string,
  language: string | null | undefined,
): Promise<ThemedToken[][] | null> {
  const resolved = resolveLanguage(language);
  if (resolved === null) return null;
  if (!withinHighlightBudget(code)) return null;
  try {
    await ensureLanguage(resolved);
    const instance = await core();
    return instance.codeToTokensBase(code, { lang: resolved, theme: THEME_NAME });
  } catch {
    return null;
  }
}

/** 记号的内联样式：只解出颜色与字形，不额外堆类名。 */
export function tokenStyle(token: ThemedToken): CSSProperties {
  const style: CSSProperties = {};
  if (typeof token.color === 'string') style.color = token.color;
  const font = typeof token.fontStyle === 'number' ? token.fontStyle : 0;
  if ((font & 1) !== 0) style.fontStyle = 'italic';
  if ((font & 2) !== 0) style.fontWeight = 600;
  if ((font & 4) !== 0) style.textDecoration = 'underline';
  return style;
}

/** 记号数组渲染成 React 节点序列的键。 */
export function tokenKey(line: number, index: number): string {
  return `${line}:${index}`;
}
