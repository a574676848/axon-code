import React from 'react';
import { Markdown } from '../../components/conversation/Markdown';
import { BodyShell } from './parts';
import type { ViewerProps } from './contract';

/*
 * 两种纯文本看法：纯文本与 Markdown。
 *
 * 两者共用同一份正文骨架（BodyShell）：顶部一行说明内容规模，下面只有一个滚动区，
 * 因此换一种看法时滚动条、左轴与换行行为都落在同一处。
 *
 * 纯文本**没有行号槽**：行号属于代码，不属于任何文本文件，与既有工作台产品同口径。
 */

/** 纯文本：整份按行铺开，行左边距与文件树的行对齐。 */
export const PlainBody: React.FC<ViewerProps> = ({ content, wrap, line }) => {
  const target = React.useRef<HTMLElement | null>(null);
  React.useEffect(() => {
    if (line === undefined || line <= 0) return;
    target.current?.scrollIntoView({ block: 'center' });
  });

  if (content.kind !== 'text') return null;
  const text = content.text.endsWith('\n') ? content.text.slice(0, -1) : content.text;
  const lines = text.split('\n');
  return (
    <BodyShell label={`纯文本 · ${lines.length} 行`}>
      <div
        className={`min-w-full px-4 py-3 font-mono text-code text-text-primary ${
          wrap ? 'whitespace-pre-wrap break-words' : 'whitespace-pre'
        }`}
      >
        {content.truncated && (
          <p className="mb-2 font-sans text-meta text-text-tertiary">内容超过 2 MiB，已截断显示。</p>
        )}
        <pre className="m-0 font-mono text-code">
          {lines.map((value, index) => (
            <div
              key={index}
              ref={line === index + 1 ? (element) => void (target.current = element) : undefined}
              data-textpreview-line={index + 1}
              className={`-mx-4 px-4${line === index + 1 ? ' bg-focus-soft' : ''}`}
            >
              {value}
              {'\n'}
            </div>
          ))}
        </pre>
      </div>
    </BodyShell>
  );
};

/**
 * Markdown：交给正文那套渲染管线，因此查看页与对话里的同一份文档是同一种排版。
 *
 * 阅读页按 13/22 排：停靠面比对话列窄，14/24 在这里会显得松散。
 * 正文组件的根节点自带 14px，因此这里对它的直接子级覆盖字号与行高，
 * 标题仍按各自的标题档，不跟着缩。
 */
export const MarkdownBody: React.FC<ViewerProps> = ({ content }) => {
  if (content.kind !== 'text') return null;
  return (
    <BodyShell label="Markdown">
      <div className="px-4 py-3 text-[13px] leading-[22px] [&>div]:text-[13px] [&>div]:leading-[22px]">
        {content.truncated && (
          <p className="mb-2 text-meta text-text-tertiary">内容超过 2 MiB，已截断显示。</p>
        )}
        <Markdown source={content.text} />
      </div>
    </BodyShell>
  );
};
