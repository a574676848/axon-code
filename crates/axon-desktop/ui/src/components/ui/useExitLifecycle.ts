import { useEffect, useRef, useState } from 'react';

/**
 * 弹出层的双向生命周期。
 *
 * `{open && <Panel />}` 的问题在于关闭是 0ms 的：弹出层突然消失，读起来像一次闪断。
 * 这里把关闭拆成两段：先进入 `closed` 态让缩放淡出跑完，再真正从 DOM 里摘掉。
 *
 * 时长取 110ms：比入场（180ms）短，因为「离开」要利落，
 * 与入场共用同一时长会让关闭显得拖沓。
 */

/** 退场时长；与 `tokens.css` 里 `.modal-spring-surface` 的过渡时长一致。 */
const EXIT_MS = 110;

/** 弹出层生命周期状态：直接写到元素的 `data-state` 上。 */
export type OverlayState = 'open' | 'closed';

export interface ExitLifecycle {
  /** 是否仍留在 DOM 里：退场动画跑完才翻成 false。 */
  mounted: boolean;
  /** 当前状态，写进 `data-state`。 */
  state: OverlayState;
}

/**
 * 计算弹出层的挂载与状态。
 *
 * 打开即刻挂载（不额外等一帧，否则会出现「点了没反应」的错觉），
 * 关闭则保留 `exitMs` 之后再卸载。
 */
export function useExitLifecycle(open: boolean, exitMs: number = EXIT_MS): ExitLifecycle {
  const [retained, setRetained] = useState(open);
  const [state, setState] = useState<OverlayState>(open ? 'open' : 'closed');
  const timer = useRef<number | null>(null);

  useEffect(() => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
    if (open) {
      setRetained(true);
      setState('open');
      return undefined;
    }
    setState('closed');
    timer.current = window.setTimeout(() => {
      timer.current = null;
      setRetained(false);
    }, exitMs);
    return () => {
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
        timer.current = null;
      }
    };
  }, [open, exitMs]);

  return {
    // 打开的那一帧立刻可见：等 effect 回来再挂载会先渲染一次「什么都没有」。
    mounted: open || retained,
    state: open ? 'open' : state,
  };
}
