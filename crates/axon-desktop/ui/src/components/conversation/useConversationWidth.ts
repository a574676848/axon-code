import React from 'react';
import {
  contentLowerBound,
  DEFAULT_WIDTH,
  HANDLE_MIN_CONTAINER_WIDTH,
  readWidthPreference,
  resolveContentWidth,
  WIDTH_PREF_KEY,
} from './widthControls';

/*
 * 对话列宽度的唯一解析点。
 *
 * 宽度是整块工作台共用的一条度量，而不是「正文区里的一个控件」：
 * 首屏、正文、输入坞、计划坞与两侧手柄读的必须是同一个 `--axon-chat-user-width`。
 * 因此这里把它提到视图层，无论当前显示的是首屏还是正文都持续解析并写入，
 * 不再挂在某一层的显隐分支上（此前挂在 cover === 'none' 上，首屏 840、
 * 打开会话 741，同窗口横跳 99px）。
 */

/** 宽度控制器：视图层持有，手柄层只负责把指针位移交回来。 */
export interface ConversationWidthController {
  /** 当前生效的列宽 (px)。窗格未量到时给出默认基准。 */
  width: number;
  /** 是否显示两侧拖拽手柄；窄窗下不予显示。 */
  handlesVisible: boolean;
  /** 拖拽起点：返回本次拖拽的基准宽度。 */
  startDrag: () => number;
  /** 拖拽进行中：按位移求解并即时应用。 */
  drag: (width: number) => void;
  /** 拖拽结束：持久化。 */
  commit: (width: number) => void;
  /** 双击手柄：清除偏好，回到默认基准。 */
  reset: () => void;
}

/**
 * 按容器宽度解析列宽并写入 CSS 变量。
 *
 * @param containerRef 量测容器（正文舞台外层）；窗口与侧栏变化都会改变它的宽度。
 * @param handlesEnabled 手柄是否被允许出现（例如过渡面期间不出现）。
 */
export function useConversationWidth(
  containerRef: React.RefObject<HTMLElement | null>,
  handlesEnabled: boolean,
): ConversationWidthController {
  const [containerWidth, setContainerWidth] = React.useState(0);
  /** 本会话内的即时宽度（拖拽中与写入偏好后都在这里）；null 表示跟随本地偏好。 */
  const [override, setOverride] = React.useState<number | null>(null);

  /*
   * 先量后用。放在绘制前量测：首帧就写对宽度，避免先按默认 840 排一次再横跳。
   */
  React.useLayoutEffect(() => {
    const container = containerRef.current;
    if (container === null) return undefined;
    const measure = () => {
      const next = container.offsetWidth;
      if (next > 0) setContainerWidth(next);
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(container);
    return () => observer.disconnect();
  }, [containerRef]);

  const width = React.useMemo(() => {
    if (containerWidth <= 0) return DEFAULT_WIDTH;
    return resolveContentWidth(
      containerWidth,
      override ?? readWidthPreference(),
      contentLowerBound(containerWidth),
    );
  }, [containerWidth, override]);

  /*
   * 写入全站变量：documentElement 是正文、输入坞、计划坞与手柄共同的祖先，
   * 一处写、处处同值。卸载时清掉，避免把上一屏的度量留在全局。
   */
  React.useLayoutEffect(() => {
    const root = document.documentElement;
    root.style.setProperty('--axon-chat-user-width', `${width}px`);
    return () => {
      root.style.removeProperty('--axon-chat-user-width');
    };
  }, [width]);

  const startDrag = React.useCallback(() => width, [width]);

  const drag = React.useCallback(
    (next: number) => {
      if (containerWidth <= 0) return;
      setOverride(
        resolveContentWidth(containerWidth, next, contentLowerBound(containerWidth)),
      );
    },
    [containerWidth],
  );

  const commit = React.useCallback(
    (next: number) => {
      if (containerWidth <= 0) return;
      const resolved = resolveContentWidth(
        containerWidth,
        next,
        contentLowerBound(containerWidth),
      );
      setOverride(resolved);
      try {
        localStorage.setItem(WIDTH_PREF_KEY, String(resolved));
      } catch {
        // localStorage 不可用时只影响下次启动的默认值，当前布局照常。
      }
    },
    [containerWidth],
  );

  const reset = React.useCallback(() => {
    try {
      localStorage.removeItem(WIDTH_PREF_KEY);
    } catch {
      // 忽略
    }
    setOverride(null);
  }, []);

  return {
    width,
    handlesVisible: handlesEnabled && containerWidth >= HANDLE_MIN_CONTAINER_WIDTH,
    startDrag,
    drag,
    commit,
    reset,
  };
}
