import { useCallback, useEffect, useMemo, useRef } from 'react';
import type { RefObject } from 'react';

/*
 * 标题跑马灯：单行标题被截断时，指针停在行上让标题匀速走到末尾。
 *
 * 口径与既有工作台产品一致：
 * - 溢出不足 8px 时不动，几个像素的位移只会被读成抖动；
 * - 速度 0.03px/ms，慢到能在移动过程中读清文字；
 * - 走到末尾即停在末尾，不来回摆动；
 * - 遵循「减少动效」偏好：直接落到末尾，不做动画；
 * - 只在 transform 与遮罩上做视觉变化，遮罩随滚动位置切换，
 *   因此两端渐隐而不是把字符硬切。
 */

/** 小于该溢出量不启动跑马灯，避免被判读为抖动。 */
const MIN_TITLE_REVEAL_PX = 8;
/** 行进速度，单位像素每毫秒。 */
const TITLE_MARQUEE_PX_PER_MS = 0.03;

/** 摆放标题滚动位置，并同步两侧渐隐遮罩的数据属性。 */
function placeTitle(title: HTMLElement, left: number, range: number): void {
  title.scrollLeft = left;
  if (left > 0) {
    title.dataset.scrolled = '';
  } else {
    delete title.dataset.scrolled;
  }
  if (left < range) {
    title.dataset.clipped = '';
  } else {
    delete title.dataset.clipped;
  }
}

/** 回到静止状态：滚回起点并清掉两侧渐隐。 */
function restTitle(title: HTMLElement): void {
  title.scrollLeft = 0;
  delete title.dataset.scrolled;
  delete title.dataset.clipped;
}

/** 跑马灯控制句柄。 */
export interface TitleMarquee {
  /** 指针进入行。 */
  enter: () => void;
  /** 指针离开行。 */
  leave: () => void;
}

/**
 * 为一个会被截断的标题元素提供跑马灯控制。
 * @param title 标题元素引用；元素不可滚动时自动不动。
 * @returns 进入与离开两个控制方法。
 */
export function useTitleMarquee(title: RefObject<HTMLElement | null>): TitleMarquee {
  const frame = useRef(0);

  useEffect(
    () => () => {
      cancelAnimationFrame(frame.current);
    },
    [],
  );

  return useMemo(
    () => ({
      enter: (): void => {
        const element = title.current;
        if (element === null) return;
        const range = element.scrollWidth - element.clientWidth;
        if (range <= MIN_TITLE_REVEAL_PX) return;
        if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
          placeTitle(element, range, range);
          return;
        }
        cancelAnimationFrame(frame.current);
        let previous: number | undefined;
        let position = 0;
        const step = (now: number): void => {
          position += previous === undefined ? 0 : (now - previous) * TITLE_MARQUEE_PX_PER_MS;
          previous = now;
          placeTitle(element, Math.min(position, range), range);
          if (position < range) frame.current = requestAnimationFrame(step);
        };
        frame.current = requestAnimationFrame(step);
      },
      leave: (): void => {
        cancelAnimationFrame(frame.current);
        const element = title.current;
        if (element === null) return;
        restTitle(element);
      },
    }),
    [title],
  );
}

/**
 * 把跑马灯控制绑定到一行的进入与离开事件上。
 * @param marquee 跑马灯句柄。
 * @returns 可直接展开到行元素上的事件属性。
 */
export function marqueeRowHandlers(marquee: TitleMarquee): {
  onPointerEnter: () => void;
  onPointerLeave: () => void;
} {
  return { onPointerEnter: marquee.enter, onPointerLeave: marquee.leave };
}

/** 供标题复用的跑马灯标记类名。 */
export const MARQUEE_TITLE_CLASS = 'marquee-title';

/** 便捷封装：返回引用、句柄与行事件属性。 */
export function useRowTitleMarquee(): {
  titleRef: RefObject<HTMLSpanElement | null>;
  handlers: { onPointerEnter: () => void; onPointerLeave: () => void };
} {
  const titleRef = useRef<HTMLSpanElement>(null);
  const marquee = useTitleMarquee(titleRef);
  const handlers = useCallback(() => marqueeRowHandlers(marquee), [marquee])();
  return { titleRef, handlers };
}
