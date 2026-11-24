import React from 'react';

/*
 * 代码高亮节流。
 *
 * 流式输出期间代码正文每来一个 token 就变一次，而全量 AST 高亮是毫秒级的同步计算：
 * 跟着每次变更跑，50 token/s 下会把主线程吃满，正文推进看起来一顿一顿的。
 *
 * 两条策略：
 * 1. **一帧至多一次**：变更先记进 ref，由 requestAnimationFrame 统一提交，
 *    因此再快的流也只会以显示屏的节奏解析。已排期的那一帧读的是最新值，
 *    因此同一帧内的多次变更不会互相丢弃；
 * 2. **未闭合围栏先不解析**：流式中的代码块还没有闭合的围栏，此时解析的是半截语法树，
 *    结果下一帧就被推翻。这一档由解析方（Markdown 的围栏切分）直接告知，
 *    调用方渲染等宽纯文本，等围栏闭合后再补一次完整高亮。
 */

/** 高亮函数签名：拿到这一版的 token 行。 */
export type HighlightFn<T> = (code: string, language: string | null | undefined) => Promise<T>;

export interface ThrottledHighlightOptions {
  /** 这一段正文还没写完（围栏未闭合）：本帧不做高亮，先按纯文本渲染。 */
  plain?: boolean;
}

/**
 * 按帧节流的高亮结果。
 *
 * 返回 null 表示「这一帧不做高亮」：调用方应当渲染纯文本，而不是等一个空数组。
 */
export function useThrottledHighlight<T>(
  code: string,
  language: string | null | undefined,
  highlight: HighlightFn<T>,
  options: ThrottledHighlightOptions = {},
): T | null {
  const [lines, setLines] = React.useState<T | null>(null);
  const plain = options.plain === true;
  const latest = React.useRef<{ code: string; language: string | null | undefined; plain: boolean }>({
    code,
    language,
    plain,
  });
  const frame = React.useRef(0);
  const alive = React.useRef(true);
  /** 请求序号：异步高亮可能乱序回来，只让最后一次落地。 */
  const request = React.useRef(0);

  React.useEffect(
    () => () => {
      alive.current = false;
      if (frame.current !== 0) window.cancelAnimationFrame(frame.current);
      frame.current = 0;
    },
    [],
  );

  React.useEffect(() => {
    latest.current = { code, language, plain };
    if (plain) {
      // 未闭合：立刻退回纯文本，且不再排新帧。
      setLines(null);
      return;
    }
    if (frame.current !== 0) return;
    frame.current = window.requestAnimationFrame(() => {
      frame.current = 0;
      const next = latest.current;
      if (!alive.current) return;
      if (next.plain) {
        setLines(null);
        return;
      }
      request.current += 1;
      const ticket = request.current;
      void highlight(next.code, next.language).then((value) => {
        // 只有最后一次请求有权落地：更早的结果回来时正文已经又变了。
        if (alive.current && ticket === request.current) setLines(value);
      });
    });
  }, [code, highlight, language, plain]);

  return lines;
}
