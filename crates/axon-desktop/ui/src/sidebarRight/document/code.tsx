import React from 'react';
import type { ThemedToken } from 'shiki/core';
import {
  highlightCode,
  languageOfPath,
  tokenStyle,
} from '../../components/conversation/codeHighlight';
import { CheckIcon, CopyIcon } from '../../icons';
import { BodyButton, BodyShell } from './parts';
import type { ViewerProps } from './contract';
import * as ipc from '../../ipc';

/*
 * 代码看法：一行「语言徽标 + 复制」压在正文上沿，下面是带行号的源码。
 *
 * 行号是**行元素上的 CSS 计数器**（见 tokens.css 的 `.code-lines`），不是 DOM 节点：
 * 勾选正文复制出来的内容里天然没有行号，也不必为每一行多渲染一个元素。
 *
 * 头部与正文共用一个骨架（BodyShell）：头部是滚动区的兄弟节点，因此滚动条从头部之下
 * 才开始，滚源头几行时语言徽标与复制按钮一直看得见。
 */

/** 复制成功的回执停留时长。 */
const COPIED_FEEDBACK_MS = 1500;

/** 行号槽的最小位数：个位数行数也不让行号贴着内容。 */
const MIN_GUTTER_DIGITS = 2;

export const CodeBody: React.FC<ViewerProps> = ({ path, content, wrap, line }) => {
  const [tokens, setTokens] = React.useState<ThemedToken[][] | null>(null);
  const [copied, setCopied] = React.useState(false);
  const target = React.useRef<HTMLElement | null>(null);

  const text = content.kind === 'text' ? content.text : '';
  const language = languageOfPath(path);

  React.useEffect(() => {
    let cancelled = false;
    setTokens(null);
    if (text.length === 0) return undefined;
    void highlightCode(text, language).then((value) => {
      if (!cancelled) setTokens(value);
    });
    return () => {
      cancelled = true;
    };
  }, [text, language]);

  React.useEffect(() => {
    if (line === undefined || line <= 0) return;
    target.current?.scrollIntoView({ block: 'center' });
  }, [line, tokens]);

  React.useEffect(() => {
    if (!copied) return undefined;
    const timer = window.setTimeout(() => setCopied(false), COPIED_FEEDBACK_MS);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const copyAll = React.useCallback(() => {
    // 复制走原生剪贴板：网页剪贴板要求窗口处于焦点，复制会时灵时不灵。
    void ipc
      .clipboardWriteText(text)
      .then(() => setCopied(true))
      .catch(() => undefined);
  }, [text]);

  const trimmed = text.endsWith('\n') ? text.slice(0, -1) : text;
  const lines = trimmed.split('\n');
  const gutter = `${Math.max(MIN_GUTTER_DIGITS, String(lines.length).length)}ch`;

  return (
    <div
      data-code-preview
      data-wrap={wrap ? 'true' : 'false'}
      className="flex-1 min-h-0 flex flex-col w-full overflow-hidden"
    >
      <BodyShell
        label={language ?? '代码块'}
        actions={
          <BodyButton label={copied ? '已复制' : '复制'} onClick={copyAll}>
            {copied ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
          </BodyButton>
        }
      >
        <pre
          className={`box-border min-w-[100%] p-4 m-0 font-mono text-code ${
            wrap ? 'whitespace-pre-wrap break-words' : 'whitespace-pre'
          }`}
        >
          <code
            className={`code-lines${wrap ? ' code-wrap' : ''}`}
            style={{ '--code-gutter-width': gutter } as React.CSSProperties}
          >
            {lines.map((value, index) => (
              <React.Fragment key={index}>
                {index > 0 && '\n'}
                <span
                  ref={line === index + 1 ? (element) => void (target.current = element) : undefined}
                  className={`line${line === index + 1 ? ' bg-focus-soft' : ''}`}
                >
                  {tokens === null || tokens[index] === undefined
                    ? value
                    : tokens[index].map((token, tokenIndex) => (
                        <span key={tokenIndex} style={tokenStyle(token)}>
                          {token.content}
                        </span>
                      ))}
                </span>
              </React.Fragment>
            ))}
          </code>
        </pre>
      </BodyShell>
    </div>
  );
};
