import React, { useCallback, useEffect, useState } from 'react';

/*
 * 跨平台沉浸式窗口外壳与右上角控制簇。
 *
 * Windows 与 Linux 在宿主层关闭系统默认标题栏（decorations: false），由本组件在右上角
 * 渲染最小化、最大化/还原、关闭三枚按钮，并通过 --window-controls-inset 让各处 40px 顶栏
 * 自动避让；macOS 保持原生 Overlay 交通灯，通过 --macos-traffic-inset 让左上角品牌区避让；
 * 浏览器调试桥下两者皆不启用，不渲染无效按钮。
 */

/** 判定当前是否运行在 Tauri 桌面宿主内。 */
function isTauriEnvironment(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** 判定当前是否为 macOS 平台。 */
function isMacPlatform(): boolean {
  if (typeof navigator === 'undefined') return false;
  return /Mac|iPhone|iPad|iPod/i.test(navigator.platform || navigator.userAgent);
}

/** 拖拽区域内不应触发窗口移动的可交互或可选文本选择器。 */
const INTERACTIVE_SELECTOR =
  'button, a, input, textarea, select, [role="button"], [role="menuitem"], [role="option"], [data-no-drag], [data-header-cluster]';

export const WindowChrome: React.FC = () => {
  const [tauriHost] = useState(isTauriEnvironment);
  const [macHost] = useState(isMacPlatform);
  const [maximized, setMaximized] = useState(false);

  const showCustomControls = tauriHost && !macHost;

  useEffect(() => {
    const root = document.documentElement;
    if (!tauriHost) {
      root.style.setProperty('--window-controls-inset', '0px');
      root.style.setProperty('--macos-traffic-inset', '0px');
      return;
    }
    if (macHost) {
      root.style.setProperty('--window-controls-inset', '0px');
      root.style.setProperty('--macos-traffic-inset', '72px');
    } else {
      root.style.setProperty('--window-controls-inset', '104px');
      root.style.setProperty('--macos-traffic-inset', '0px');
    }
  }, [tauriHost, macHost]);

  // 同步最大化状态，并委托顶栏空白处的拖拽与双击缩放。
  useEffect(() => {
    if (!tauriHost) return undefined;
    let unlistenResize: (() => void) | null = null;
    let cancelled = false;

    void import('@tauri-apps/api/window')
      .then(async ({ getCurrentWindow }) => {
        if (cancelled) return;
        const win = getCurrentWindow();
        setMaximized(await win.isMaximized().catch(() => false));
        const unlisten = await win.onResized(async () => {
          if (!cancelled) {
            setMaximized(await win.isMaximized().catch(() => false));
          }
        });
        if (cancelled) unlisten();
        else unlistenResize = unlisten;
      })
      .catch(() => undefined);

    const handleMouseDown = (event: MouseEvent) => {
      if (event.button !== 0) return;
      const target = event.target;
      if (!(target instanceof Element)) return;
      const dragRegion = target.closest('[data-tauri-drag-region]');
      if (dragRegion === null) return;
      if (target.closest(INTERACTIVE_SELECTOR) !== null) return;
      void import('@tauri-apps/api/window')
        .then(({ getCurrentWindow }) => {
          const win = getCurrentWindow();
          if (event.detail === 2) {
            void win.toggleMaximize();
          } else {
            void win.startDragging();
          }
        })
        .catch(() => undefined);
    };

    window.addEventListener('mousedown', handleMouseDown);
    return () => {
      cancelled = true;
      unlistenResize?.();
      window.removeEventListener('mousedown', handleMouseDown);
    };
  }, [tauriHost]);

  const handleMinimize = useCallback(() => {
    void import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => getCurrentWindow().minimize())
      .catch(() => undefined);
  }, []);

  const handleToggleMaximize = useCallback(() => {
    void import('@tauri-apps/api/window')
      .then(async ({ getCurrentWindow }) => {
        const win = getCurrentWindow();
        await win.toggleMaximize();
        setMaximized(await win.isMaximized().catch(() => false));
      })
      .catch(() => undefined);
  }, []);

  const handleClose = useCallback(() => {
    void import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => getCurrentWindow().close())
      .catch(() => undefined);
  }, []);

  if (!showCustomControls) return null;

  return (
    <div
      data-no-drag
      aria-label="窗口控制"
      className="fixed top-1.5 right-2.5 z-[950] h-10 flex items-center gap-0.5 select-none"
    >
      <button
        type="button"
        aria-label="最小化窗口"
        title="最小化"
        onClick={handleMinimize}
        className="w-8 h-7 rounded-sm inline-flex items-center justify-center text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
          <path d="M1.5 5H8.5" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
        </svg>
      </button>
      <button
        type="button"
        aria-label={maximized ? '向下还原窗口' : '最大化窗口'}
        title={maximized ? '向下还原' : '最大化'}
        onClick={handleToggleMaximize}
        className="w-8 h-7 rounded-sm inline-flex items-center justify-center text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
      >
        {maximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
            <rect
              x="1.5"
              y="3"
              width="5.5"
              height="5.5"
              rx="1"
              stroke="currentColor"
              strokeWidth="1.1"
            />
            <path
              d="M3.2 3V2.2C3.2 1.8 3.5 1.5 3.9 1.5H7.8C8.2 1.5 8.5 1.8 8.5 2.2V6.1C8.5 6.5 8.2 6.8 7.8 6.8H7"
              stroke="currentColor"
              strokeWidth="1.1"
              strokeLinecap="round"
            />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
            <rect
              x="1.75"
              y="1.75"
              width="6.5"
              height="6.5"
              rx="1.2"
              stroke="currentColor"
              strokeWidth="1.2"
            />
          </svg>
        )}
      </button>
      <button
        type="button"
        aria-label="关闭窗口"
        title="关闭"
        onClick={handleClose}
        className="w-8 h-7 rounded-sm inline-flex items-center justify-center text-text-secondary transition-colors duration-100 hover:bg-window-close hover:text-white focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
          <path
            d="M2 2L8 8M8 2L2 8"
            stroke="currentColor"
            strokeWidth="1.2"
            strokeLinecap="round"
          />
        </svg>
      </button>
    </div>
  );
};
