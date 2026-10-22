import React from 'react';
import type { ThemedToken } from 'shiki/core';
import { CheckIcon, CopyIcon } from '../../icons';
import * as ipc from '../../ipc';
import { highlightCode, tokenStyle } from './codeHighlight';
import { MermaidDiagram } from './MermaidDiagram';

/*
 * 正文里的代码块：外框、语言条与着色统一在这里实现。
 *
 * 三种形态共用一套外观：能高亮的高亮显示，识别不了语言的按纯文本显示，
 * mermaid 定义则渲染成图形；任何一步失败都退回源码，读者永远看得到内容。
 *
 * 语言条固定 36px：左边是语言徽标（判不出语言就写「代码」），右边是 24px 见方的
 * 复制入口，回执停留 1.5s。条与正文同一块底色，只靠一条发丝线分家。
 */

/** 判不出语言时的徽标文案。 */
const UNKNOWN_LANGUAGE = '代码';
/** 复制回执的停留时长。 */
const COPY_RECEIPT_MS = 1500;

export interface CodeBlockProps {
  /** 代码正文。 */
  code: string;
  /** 围栏标记或扩展名；为空表示未标注语言。 */
  language?: string | null;
  /** 工具详情使用紧凑间距和内部滚动。 */
  compact?: boolean;
}

function isDiagram(language: string | null | undefined): boolean {
  return (language ?? '').trim().toLowerCase() === 'mermaid';
}

/**
 * 常见围栏名的中文显示名。
 *
 * 代码围栏里写的是语言标识（text、sh、ts），直接印出来是给机器看的；
 * 只翻译最常见的几种，其余保持原样，避免自造语言名。
 */
const LANGUAGE_LABELS: Record<string, string> = {
  text: '文本',
  plaintext: '文本',
  plain: '文本',
  md: 'Markdown',
  markdown: 'Markdown',
  js: 'JavaScript',
  ts: 'TypeScript',
  jsx: 'JSX',
  tsx: 'TSX',
  sh: 'Shell',
  bash: 'Shell',
  zsh: 'Shell',
  shell: 'Shell',
  py: 'Python',
  rs: 'Rust',
  json: 'JSON',
  yaml: 'YAML',
  yml: 'YAML',
  html: 'HTML',
  css: 'CSS',
  sql: 'SQL',
};

/** 语言徽标：判不出语言时给「代码」，而不是留一条空条。 */
function languageLabel(language: string | null | undefined): string {
  const label = (language ?? '').trim();
  if (label.length === 0) return UNKNOWN_LANGUAGE;
  return LANGUAGE_LABELS[label.toLowerCase()] ?? label;
}

export const CodeBlock: React.FC<CodeBlockProps> = ({ code, language, compact = false }) => {
  const [lines, setLines] = React.useState<ThemedToken[][] | null>(null);
  const [diagramError, setDiagramError] = React.useState<string | null>(null);
  const [copied, setCopied] = React.useState(false);
  const receiptTimer = React.useRef<number | null>(null);

  React.useEffect(() => {
    let cancelled = false;
    setLines(null);
    setDiagramError(null);
    void highlightCode(code, language).then((value) => {
      if (!cancelled) setLines(value);
    });
    return () => {
      cancelled = true;
    };
  }, [code, language]);

  React.useEffect(
    () => () => {
      if (receiptTimer.current !== null) window.clearTimeout(receiptTimer.current);
    },
    [],
  );

  const copy = () => {
    // 复制走原生剪贴板：网页剪贴板要求窗口处于焦点，复制会时灵时不灵。
    void ipc.clipboardWriteText(code).then(
      () => {
        setCopied(true);
        if (receiptTimer.current !== null) window.clearTimeout(receiptTimer.current);
        receiptTimer.current = window.setTimeout(() => setCopied(false), COPY_RECEIPT_MS);
      },
      () => undefined,
    );
  };

  const label = languageLabel(language);

  if (isDiagram(language) && diagramError === null) {
    return (
      <div className="my-4 rounded-md hairline border-border bg-code-block overflow-hidden">
        <MermaidDiagram code={code} onError={setDiagramError} />
      </div>
    );
  }

  return (
    <div
      className={`${compact ? 'my-0' : 'my-4'} rounded-md hairline border-border bg-code-block overflow-hidden`}
    >
      <div className="h-9 pl-4 pr-1.5 flex items-center gap-2 hairline-b border-border bg-code-block">
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-text-tertiary">
          {diagramError === null ? label : `${label}（图形渲染失败，显示原始定义）`}
        </span>
        <button
          type="button"
          aria-label={copied ? '已复制代码' : '复制代码'}
          title={copied ? '已复制' : '复制代码'}
          onClick={copy}
          className={`shrink-0 w-6 h-6 inline-flex items-center justify-center rounded-xs transition-colors duration-100 hover:bg-hover hover:text-text-primary ${
            copied ? 'text-ok-ink' : 'text-text-tertiary'
          }`}
        >
          {copied ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
        </button>
      </div>
      <pre
        className={`${
          compact
            ? 'px-3 py-2 max-h-80 overflow-auto whitespace-pre-wrap break-words'
            : 'px-4 py-3 overflow-x-auto'
        } font-mono text-code text-text-primary`}
      >
        <code>
          {lines === null
            ? code
            : lines.map((tokens, lineIndex) => (
                <React.Fragment key={lineIndex}>
                  {lineIndex > 0 ? '\n' : null}
                  {tokens.map((token, tokenIndex) => (
                    <span key={tokenIndex} style={tokenStyle(token)}>
                      {token.content}
                    </span>
                  ))}
                </React.Fragment>
              ))}
        </code>
      </pre>
    </div>
  );
};
