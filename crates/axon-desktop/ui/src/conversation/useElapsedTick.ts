import { useEffect } from 'react';

/*
 * 运行中刷新耗时的节拍。
 *
 * 单独成文：它是纯时钟，不属于会话编排的任何一段流程。派发只走 `tick`，
 * 归约器据此重算进行中轮次的耗时，节拍本身不持有任何事实。
 *
 * 只在真有轮次在跑时起表：空闲时的每一拍都会让整棵会话重渲染一次，
 * 而那时没有任何读数会变。
 */

/** 运行中刷新耗时的节拍。 */
const TICK_MS = 500;

/**
 * 按节拍派发一次 `tick`；`active` 为假时不起表。
 *
 * `dispatch` 由调用方给：本 hook 不创建归约器，只负责发心跳。
 */
export function useElapsedTick(
  dispatch: (action: { type: 'tick'; nowMs: number }) => void,
  active: boolean,
): void {
  useEffect(() => {
    if (!active) return undefined;
    const timer = window.setInterval(() => dispatch({ type: 'tick', nowMs: Date.now() }), TICK_MS);
    return () => window.clearInterval(timer);
  }, [dispatch, active]);
}
