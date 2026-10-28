import React from 'react';

/*
 * 代码高亮节流。
 *
 * 流式输出期间代码正文每来一个 token 就变一次，而全量 AST 高亮是毫秒级的同步计算：
 * 跟着每次变更跑，50 token/s 下会把主线程吃满，正文推进看起来一顿一顿的。
 *
 * 两条策略：
 * 1. **一帧至多一次**：变更先记进 ref，由 requestAnimationFrame 统一提交，
 *    因此再快的流也只会以显示屏的节奏解析；
 * 2. **未闭合围栏先不解析**：流式中的代码块还没有闭合的 ```，此时解析的是半截语法树，
 *    结果下一帧就被推翻。这一档先返回 null（由调用方退回等宽纯文本），
 *    等围栏闭合、正文稳定后再补一次完整高亮。
 */

/** 高亮函数签名：拿到 token 行或 null（null 表示暂时不做高亮）。 */
export type HighlightFn<T> = (code: string, language: string | null | undefined) => Promise<T>;

/** 正文里是否还有没闭合的代码围栏。 */
export function hasUnclosedFence(code: string): boolean {
  let fences = 0;
  for (const line of code.split('\n')) {
    if (line.trimStart().startsWith('```')) fences += 1;
  }
  return fences % 2 === 1;
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
): T | null {
  const [lines, setLines] = React.useState<T | null>(null);
  const pending = React.useRef<{ code: string; language: string | null | undefined } | null>(null);
  const frame = React.useRef(0);

  React.useEffect(() => {
    let cancelled = false;
    pending.current = { code, language };
    if (frame.current === 0) {
      frame.current = window.requestAnimationFrame(() => {
        frame.current = 0;
        const next = pending.current;
        if (next === null) return;
        if (hasUnclosedFence(next.code)) {
          setLines(null);
          return;
        }
        void highlight(next.code, next.language).then((value) => {
          if (!cancelled) setLines(value);
        });
      });
    }
    return () => {
      cancelled = true;
    };
  }, [code, highlight, language]);

  React.useEffect(
    () => () => {
      if (frame.current !== 0) window.cancelAnimationFrame(frame.current);
    },
    [],
  );

  return lines;
}
