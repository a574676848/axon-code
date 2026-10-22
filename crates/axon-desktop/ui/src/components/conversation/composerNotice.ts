import React from 'react';

/*
 * 输入坞上方那一行提示的存活时长与退场机制。
 *
 * 这一行说的是「刚才这一次操作怎么样了」，是一次性回执而不是页面状态：失败的事实
 * 源在正文里（未送达标记、失败卡与重新发送入口），过程说明本来也不必留档。因此它
 * 到点自己退场，不长期占着输入框上方的位置、把接下来要读的正文一直往下挤。
 */

/** 过程说明的停留时长：看一眼就够。 */
export const NOTICE_MS = 3200;
/** 失败提示的停留时长：留够读完一句原因的时间。 */
export const NOTICE_FAILURE_MS = 6000;

/**
 * 让一条提示自己退场：`message` 非空时开始计时，到点调用 `clear` 把状态交还源头。
 *
 * `clear` 经 ref 取当前值，不进依赖数组：调用方多数直接传内联回调，每次重渲染都会
 * 换一个新引用，进依赖数组就等于每渲染一次重新计时，提示永远等不到退场那一刻。
 * 因此重新计时只有一个触发点，就是提示内容本身变了。
 */
export function useExpiringNotice(
  message: string | null,
  clear: () => void,
  ms: number = NOTICE_FAILURE_MS,
): void {
  const clearRef = React.useRef(clear);
  clearRef.current = clear;
  React.useEffect(() => {
    if (message === null) return undefined;
    const timer = window.setTimeout(() => clearRef.current(), ms);
    return () => window.clearTimeout(timer);
  }, [message, ms]);
}
