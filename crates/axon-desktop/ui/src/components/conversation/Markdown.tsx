import React from 'react';
import { useConversationActions } from './actions';
import { CodeBlock } from './CodeBlock';
import { CheckIcon } from '../../icons';
import { renderInline, type MarkdownArt } from './MarkdownInline';
import { isImagePath } from './localImage';

/*
 * 极简 Markdown 渲染器。
 *
 * 只覆盖助手正文实际会用到的语法子集：段落、标题、列表、引用、围栏代码块与行内代码，
 * 以及粗体、斜体、链接与图片。刻意不引入第三方解析库：桌面端要离线自足，
 * 而完整 Markdown 规范里的大多数构造在对话正文中并不出现。
 *
 * 行内标记的渲染在 MarkdownInline 里：图片、链接与文件路径芯片三者互相牵扯
 * （图片语法本身长得像链接），拆开就再也说不清谁先谁后。
 */

/** 整段只有一张图片的判据：这种段落自带间距，不再套一层段落下边距。 */
const IMAGE_BLOCK_HINT = /^\s*!\[[^\]]*\]\([^)]+\)\s*$/;

/** 段落是不是整段只有一张图片。 */
function isImageOnlyParagraph(text: string): boolean {
  return IMAGE_BLOCK_HINT.test(text);
}

/** 列表项是不是整项只有一张图片或一个图片路径。 */
function isImageOnlyItem(text: string): boolean {
  return IMAGE_BLOCK_HINT.test(text) || isImagePath(text.trim());
}

interface Block {
  kind: 'paragraph' | 'heading' | 'list' | 'quote' | 'code' | 'table' | 'divider';
  level?: number;
  language?: string;
  ordered?: boolean;
  items?: string[];
  /** 表格表头与数据行；第一行是表头。 */
  rows?: string[][];
  /** 任务列表每项的勾选状态；非任务列表时缺省。 */
  taskChecks?: boolean[];
  /** 每个列表项的嵌套层级；与 items 等长。 */
  /** 代码围栏是否已经闭合；流式输出中未闭合时先按纯文本渲染。 */
  closed?: boolean;
  depths?: number[];
  text: string;
}

const FENCE = '```';

/** 列表嵌套层级上限：再深也只是缩进，没必要让正文被推到屏幕外。 */
const MAX_LIST_DEPTH = 4;

/** 每一级嵌套的缩进像素。 */
const LIST_INDENT_PX = 18;

/** 表格分隔行：形如 `| --- | :--: |`，用于确认上一行是表头。 */
const TABLE_DIVIDER = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

/** 拆分一行表格；两侧的空竖线会被丢弃。 */
function splitTableRow(line: string): string[] {
  const trimmed = line.trim().replace(/^\|/, '').replace(/\|$/, '');
  return trimmed.split('|').map((cell) => cell.trim());
}

/** 把源文本切成块级结构。 */
function parseBlocks(source: string): Block[] {
  const lines = source.replaceAll('\r\n', '\n').split('\n');
  const blocks: Block[] = [];
  let paragraph: string[] = [];

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    blocks.push({ kind: 'paragraph', text: paragraph.join('\n') });
    paragraph = [];
  };

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trimStart().startsWith(FENCE)) {
      flushParagraph();
      const language = line.trim().slice(FENCE.length).trim();
      const body: string[] = [];
      index += 1;
      while (index < lines.length && !lines[index].trimStart().startsWith(FENCE)) {
        body.push(lines[index]);
        index += 1;
      }
      /*
       * 围栏是否闭合要在这里判定：while 循环只有撞到收尾围栏才会提前退出，
       * 遍历到末尾仍未撞到，说明这是流式输出里还没写完的那一块。
       */
      blocks.push({ kind: 'code', language, text: body.join('\n'), closed: index < lines.length });
      continue;
    }
    /*
     * 标题容忍至多 3 个前导空格（ATX 标题的通用口径）：模型偶尔在 `###` 前多打一个
     * 空格，锚定第 0 列会让整行掉进普通段落、把井号原样显示出来。上限取 3 而不是
     * 任意宽度，是为了让 4 空格缩进的样张留在段落里按原文呈现，不被吃成标题。
     */
    const heading = /^ {0,3}(#{1,4})\s+(.*)$/.exec(line);
    if (heading !== null) {
      flushParagraph();
      blocks.push({ kind: 'heading', level: heading[1].length, text: heading[2] });
      continue;
    }
    if (/^\s*>\s?/.test(line)) {
      flushParagraph();
      const body: string[] = [];
      while (index < lines.length && /^\s*>\s?/.test(lines[index])) {
        body.push(lines[index].replace(/^\s*>\s?/, ''));
        index += 1;
      }
      index -= 1;
      blocks.push({ kind: 'quote', text: body.join('\n') });
      continue;
    }
    // 表格：当前行是行首竖线行，且下一行是分隔行时才算表格，
    // 否则普通文本里的竖线会被误判成表格。
    if (
      line.includes('|') &&
      index + 1 < lines.length &&
      TABLE_DIVIDER.test(lines[index + 1])
    ) {
      flushParagraph();
      const rows: string[][] = [splitTableRow(line)];
      index += 2;
      while (index < lines.length && lines[index].includes('|') && lines[index].trim() !== '') {
        rows.push(splitTableRow(lines[index]));
        index += 1;
      }
      index -= 1;
      blocks.push({ kind: 'table', rows, text: '' });
      continue;
    }
    // 分割线：三个及以上的 `-`、`*`、`_` 独占一行。
    // 必须放在列表判断之前：`---` 同时也满足无序列表的前缀规则。
    if (/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      flushParagraph();
      blocks.push({ kind: 'divider', text: '' });
      continue;
    }
    const bullet = /^\s*[-*+]\s+/.test(line);
    const ordered = /^\s*\d+[.)]\s+/.test(line);
    if (bullet || ordered) {
      flushParagraph();
      const items: string[] = [];
      const checked: boolean[] = [];
      const depths: number[] = [];
      while (
        index < lines.length &&
        (bullet ? /^\s*[-*+]\s+/.test(lines[index]) : /^\s*\d+[.)]\s+/.test(lines[index]))
      ) {
        const current = lines[index];
        // 缩进层级按两个空格算一级，制表符按一级：这是 agent 输出里最常见的两种写法。
        const indent = /^[\t ]*/.exec(current)?.[0] ?? '';
        const depth = Math.min(
          MAX_LIST_DEPTH,
          indent.split('\t').length - 1 + Math.floor(indent.replace(/\t/g, '').length / 2),
        );
        const raw = current.replace(/^\s*(?:[-*+]|\d+[.)])\s+/, '');
        // 任务列表：`- [ ]` 未完成、`- [x]` 已完成；标记从正文里摘掉，改由勾选框呈现。
        const task = /^\[([ xX])\]\s*(.*)$/.exec(raw);
        if (task !== null) {
          checked.push(task[1].toLowerCase() === 'x');
          items.push(task[2]);
        } else {
          checked.push(false);
          items.push(raw);
        }
        depths.push(depth);
        index += 1;
      }
      index -= 1;
      const hasTask = checked.some((value) => value);
      blocks.push({
        kind: 'list',
        ordered,
        items,
        taskChecks: hasTask ? checked : undefined,
        depths,
        text: '',
      });
      continue;
    }
    if (line.trim().length === 0) {
      flushParagraph();
      continue;
    }
    paragraph.push(line);
  }
  flushParagraph();
  return blocks;
}

/** 渲染一段 Markdown。 */
export const Markdown: React.FC<{ source: string; streaming?: boolean }> = ({
  source,
  streaming = false,
}) => {
  const blocks = React.useMemo(() => parseBlocks(source), [source]);
  const actions = useConversationActions();
  /*
   * 记录首次挂载或非流式稳态下的块数基线：
   * 仅在流式输出（streaming=true）过程中新追加出来的块挂载柔光浮现动画，
   * 历史回放与已完成消息直接稳态呈现，不产生整屏闪烁。
   */
  const baselineCountRef = React.useRef<number>(streaming ? 0 : blocks.length);
  React.useEffect(() => {
    if (!streaming) {
      baselineCountRef.current = blocks.length;
    }
  }, [streaming, blocks.length]);

  /*
   * 行内渲染需要的外界能力。
   *
   * 工作区根目录与取图通道一起构成「正文里的图片与路径」这条链：相对路径靠前者
   * 落到工作区，字节靠后者取回。正文之外调用（右侧栏的文档预览）拿不到会话上下文，
   * 因此图片退化成 alt 文本、路径仍可点开。
   */
  const art = React.useMemo<MarkdownArt>(
    () => ({
      openFile: actions?.openFile,
      workspaceRoot: actions?.workspaceRoot ?? null,
      hoverThumbnail: actions !== null,
    }),
    [actions],
  );
  const render = React.useCallback(
    (text: string, key: string) => renderInline(text, key, art),
    [art],
  );
  return (
    <div className="text-content text-text-prose">
      {blocks.map((block, index) => {
        const key = `block:${index}`;
        const enterClass = streaming && index >= baselineCountRef.current ? 'stream-block-in' : '';
        if (block.kind === 'code') {
          return (
            <div key={key} className={enterClass || undefined}>
              <CodeBlock
                code={block.text}
                language={block.language ?? null}
                streaming={streaming && block.closed === false}
              />
            </div>
          );
        }
        if (block.kind === 'heading') {
          const level = block.level ?? 1;
          const size =
            level === 1
              ? 'text-heading-1 font-bold tracking-[-0.02em] text-text-primary'
              : level === 2
                ? 'text-heading-3 font-bold tracking-[-0.015em] text-text-primary'
                : level === 3
                  ? 'text-heading-4 font-semibold tracking-[-0.01em] text-text-primary'
                  : 'text-content font-semibold text-text-primary';
          const spacing =
            level <= 2 ? 'mt-6 mb-3' : level === 3 ? 'mt-4 mb-2' : 'mt-4 mb-1';
          return (
            <p key={key} className={`${spacing} ${size} ${enterClass}`}>
              {render(block.text, key)}
            </p>
          );
        }
        if (block.kind === 'quote') {
          return (
            <blockquote
              key={key}
              className={`my-4 rounded-r-sm border-l-[3px] border-border-selected pl-4 pr-3 py-1.5 bg-surface-sunken/60 text-text-secondary ${enterClass}`}
            >
              {block.text.split('\n').map((line, lineIndex) => (
                <p key={`${key}:${lineIndex}`}>{render(line, `${key}:${lineIndex}`)}</p>
              ))}
            </blockquote>
          );
        }
        if (block.kind === 'list') {
          const items = block.items ?? [];
          const ListTag = block.ordered === true ? 'ol' : 'ul';
          return (
            <ListTag
              key={key}
              className={`my-4 pl-6 space-y-2.5 marker:text-text-tertiary ${
                block.ordered === true ? 'list-decimal' : 'list-disc'
              } ${enterClass}`}
            >
              {items.map((item, itemIndex) => (
                <li
                  key={`${key}:${itemIndex}`}
                  className={
                    block.taskChecks === undefined ? '' : 'flex items-start gap-2 list-none'
                  }
                  style={{
                    marginLeft: `${(block.depths?.[itemIndex] ?? 0) * LIST_INDENT_PX}px`,
                  }}
                >
                  {block.taskChecks !== undefined && (
                    <span
                      aria-hidden="true"
                      className={`shrink-0 mt-[4px] w-3.5 h-3.5 rounded-xs hairline flex items-center justify-center ${
                        block.taskChecks[itemIndex] === true
                          ? 'bg-primary border-primary text-white'
                          : 'border-border-strong'
                      }`}
                    >
                      {block.taskChecks[itemIndex] === true && <CheckIcon size={12} />}
                    </span>
                  )}
                  {isImageOnlyItem(item) ? (
                    <div className="min-w-0">{render(item, `${key}:${itemIndex}`)}</div>
                  ) : (
                    <span className="min-w-0">{render(item, `${key}:${itemIndex}`)}</span>
                  )}
                </li>
              ))}
            </ListTag>
          );
        }
        if (block.kind === 'table') {
          const rows = block.rows ?? [];
          const [header, ...body] = rows;
          return (
            <div key={key} className={`my-4 overflow-x-auto ${enterClass}`}>
              <table className="w-full border-collapse text-row">
                <thead>
                  <tr>
                    {(header ?? []).map((cell, cellIndex) => (
                      <th
                        key={`${key}:h:${cellIndex}`}
                        className="px-3 py-1.5 text-left font-medium text-text-primary hairline-b border-border-strong"
                      >
                        {render(cell, `${key}:h:${cellIndex}`)}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {body.map((row, rowIndex) => (
                    <tr key={`${key}:r:${rowIndex}`}>
                      {row.map((cell, cellIndex) => (
                        <td
                          key={`${key}:r:${rowIndex}:${cellIndex}`}
                          className="px-3 py-1.5 align-top text-text-prose hairline-b border-border-hairline"
                        >
                          {render(cell, `${key}:r:${rowIndex}:${cellIndex}`)}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          );
        }
        if (block.kind === 'divider') {
          return <hr key={key} className="my-7 border-0 h-hairline bg-border" />;
        }
        if (isImageOnlyParagraph(block.text)) {
          return <div key={key} className={enterClass || undefined}>{render(block.text, key)}</div>;
        }
        return (
          <p key={key} className={`my-3.5 whitespace-pre-wrap break-words ${enterClass}`}>
            {render(block.text, key)}
          </p>
        );
      })}
    </div>
  );
};
