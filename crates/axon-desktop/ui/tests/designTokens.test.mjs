import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative } from 'node:path';

/*
 * 设计令牌的静态校验。
 *
 * 令牌名写错不会报错：Tailwind 不生成这个工具类，浏览器也不认这个自定义属性，
 * 样式整段静默失效，界面「看起来还行」但层级与对比度已经不对了。因此这里把
 * 《工作台设计系统》8.1 的规则落到源码上：组件引用的令牌必须真实存在，
 * 颜色不得以字面量写进组件，琥珀只用于标记与描边。
 */

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = join(here, '..', 'src');
const tokensText = readFileSync(join(srcRoot, 'tokens.css'), 'utf8');

/** tokens.css 里声明的全部自定义属性。 */
const DECLARED = new Set(
  [...tokensText.matchAll(/(--[a-z0-9-]+)\s*:/g)].map((match) => match[1]),
);

/** 运行时才给出的变量：框架内部量、高亮引擎的逐记号变量、按实例内联的排版量。 */
const RUNTIME_VARS = ['--tw-', '--shiki-', '--code-'];

/** 问答三件套：UQ-103 的验收面，颜色与字号只允许来自令牌。 */
const QUESTION_COMPONENTS = [
  'components/conversation/ComposerQuestionCard.tsx',
  'components/conversation/QuestionCellBlock.tsx',
  'components/conversation/QuestionCountdown.tsx',
];

/** 琥珀标记档的合理去处：整颗状态点取 currentColor，不是文字。 */
const AMBER_MARK_FILES = new Set(['components/ui/surfaces.tsx']);

function sourceFiles(dir = srcRoot) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sourceFiles(path);
    return /\.tsx?$/.test(path) ? [path] : [];
  });
}

function relativePath(path) {
  return relative(srcRoot, path).split('\\').join('/');
}

/** JSX 里出现过的类名片段（含模板字符串与三元分支）。 */
function classNameFragments(text) {
  const fragments = [];
  for (const match of text.matchAll(/className=(?:"([^"]*)"|\{`([^`]*)`\}|\{([^}]*)\})/g)) {
    fragments.push(match[1] ?? match[2] ?? match[3] ?? '');
  }
  return fragments;
}

test('组件引用的设计令牌全部在 tokens.css 里有定义', () => {
  const missing = [];
  for (const path of sourceFiles()) {
    const text = readFileSync(path, 'utf8');
    for (const match of text.matchAll(/var\(\s*(--[a-z0-9-]+)/g)) {
      const name = match[1];
      if (RUNTIME_VARS.some((prefix) => name.startsWith(prefix))) continue;
      if (!DECLARED.has(name)) missing.push(`${relativePath(path)}: ${name}`);
    }
  }
  assert.deepEqual(missing, [], `以下令牌没有定义，样式会静默失效：\n${missing.join('\n')}`);
});

test('类名里的颜色角色全部能落到真实令牌上', () => {
  // 非令牌的内建工具类：颜色关键字、排版方向与尺寸档、边框与装饰的取值关键字。
  const BUILTIN = new Set([
    'white', 'black', 'transparent', 'current', 'inherit', 'none', 'auto',
    'left', 'center', 'right', 'justify', 'start', 'end',
    'ellipsis', 'clip', 'wrap', 'nowrap', 'balance', 'pretty',
    'xs', 'sm', 'base', 'lg', 'xl', '2xl', '3xl', '4xl', '5xl',
    'collapse', 'separate', 'solid', 'dashed', 'dotted', 'double', 'wavy',
    't', 'b', 'l', 'r', 'x', 'y', 's', 'e',
  ]);
  // SVG 呈现属性名与边框/轮廓的几何后缀，它们不是颜色角色。
  const NOT_A_COLOR = /^(width|opacity|rule|linecap|linejoin|dasharray|dashoffset|anchor|spacing|offset)/;
  const PREFIX = /^(bg|text|border|fill|stroke|outline|ring|placeholder|accent|caret|decoration)-(.+)$/;

  const missing = [];
  for (const path of sourceFiles()) {
    for (const fragment of classNameFragments(readFileSync(path, 'utf8'))) {
      for (const piece of fragment.split(/\s+/)) {
        // 去掉变体前缀（hover: / focus-visible: / placeholder: 等）与透明度后缀。
        const token = piece.replace(/^[a-z-]+(?::[a-z-]+)*:/, '').split('/')[0].replace(/['"`}]+$/, '');
        const match = PREFIX.exec(token);
        if (match === null) continue;
        let name = match[2];
        const direction = /^[tblrxyse]-(.+)$/.exec(name);
        if (direction !== null) name = direction[1];
        if (BUILTIN.has(name) || NOT_A_COLOR.test(name) || /^[[\d]/.test(name)) continue;
        if (DECLARED.has(`--color-${name}`) || DECLARED.has(`--text-${name}`)) continue;
        missing.push(`${relativePath(path)}: ${token}`);
      }
    }
  }
  const unique = [...new Set(missing)];
  assert.deepEqual(unique, [], `以下类名没有对应令牌，样式会静默失效：\n${unique.join('\n')}`);
});

test('琥珀只用于标记与描边，文字一律取墨水档', () => {
  const offenders = [];
  for (const path of sourceFiles()) {
    const text = readFileSync(path, 'utf8');
    const relativeName = relativePath(path);
    // 内联样式里的文字色：标记档对白底只有 2.15:1，达不到正文 AA。
    if (/color:\s*'var\(--color-gold\)'/.test(text)) {
      offenders.push(`${relativeName}: 用标记档当文字色`);
    }
    if (AMBER_MARK_FILES.has(relativeName)) continue;
    for (const fragment of classNameFragments(text)) {
      for (const piece of fragment.split(/\s+/)) {
        if (/(^|:)text-gold$/.test(piece)) offenders.push(`${relativeName}: ${piece}`);
      }
    }
  }
  assert.deepEqual(offenders, [], `琥珀文字必须用 --color-gold-ink：\n${offenders.join('\n')}`);
});

test('问答组件的样式里没有颜色字面量', () => {
  const offenders = [];
  for (const name of QUESTION_COMPONENTS) {
    const text = readFileSync(join(srcRoot, name), 'utf8');
    for (const match of text.matchAll(/#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(/g)) {
      offenders.push(`${name}: ${match[0]}`);
    }
  }
  assert.deepEqual(offenders, [], `颜色必须来自令牌：\n${offenders.join('\n')}`);
});
