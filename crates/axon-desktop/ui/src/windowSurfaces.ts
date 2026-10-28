import { useEffect, useState } from 'react';

/*
 * 桌面窗口的动态表面：系统材质结论与焦点状态机。
 *
 * 两条纪律：
 * 1. **作用域下沉**。焦点与最大化属性只挂在侧栏容器 `.app-shell-sidebar` 上，
 *    不挂 `:root`。长会话里上千个节点不该因为一次窗口失焦而全树重算样式。
 * 2. **事件来自宿主**。最大化状态由 Rust 侧在一次翻转里推一次事件，
 *    前端不做 resize 轮询，也不每个尺寸变化都问一次 `isMaximized()`。
 *
 * 材质结论由首帧就绪那一次 IPC 一并带回：窗口在同一次往返里完成呈现与上色决策，
 * 不会先画实色再切成半透。
 */

/** 宿主推送的最大化状态事件名，与 Rust 侧 `MAXIMIZED_EVENT` 一致。 */
const MAXIMIZED_EVENT = 'axon://window-maximized-changed';

/** 侧栏容器类名：焦点与最大化属性都落在这个节点上。 */
export const APP_SHELL_SIDEBAR = '.app-shell-sidebar';

/** 系统材质标识；`solid` 表示宿主没有可用材质。 */
export type WindowMaterial = 'mica' | 'vibrancy' | 'solid';

/** 归一化宿主回执的材质标识。 */
function normalizeMaterial(raw: unknown): WindowMaterial {
  return raw === 'mica' || raw === 'vibrancy' ? raw : 'solid';
}

/** 是否运行在 Tauri 桌面宿主内。 */
function isDesktopHost(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * 把材质结论落到根节点。
 *
 * `data-material` 只在真有材质时挂上，`data-material-fallback="solid"` 则显式记录降级，
 * 两者互斥：CSS 侧据此决定托盘取半透还是 100% 实色。默认（都不挂）也是实色，
 * 因此任何一条失败路径都不会留下半透的界面。
 */
function applyMaterial(material: WindowMaterial): void {
  const root = document.documentElement;
  if (material === 'solid') {
    delete root.dataset.material;
    root.dataset.materialFallback = 'solid';
    return;
  }
  root.dataset.material = material;
  delete root.dataset.materialFallback;
}

/**
 * 首帧就绪：显示窗口并把材质结论交回界面。
 *
 * 调用时机是首帧渲染之后（见 App 的挂载 effect）：宿主只有在收到这条信号
 * 或 350ms 兜底超时之后才 show，因此不会出现白底闪烁与窗口瞬移。
 */
export async function notifyDesktopReady(): Promise<WindowMaterial> {
  if (!isDesktopHost()) {
    applyMaterial('solid');
    return 'solid';
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    const material = normalizeMaterial(await invoke<string>('desktop_notify_ready'));
    applyMaterial(material);
    return material;
  } catch {
    // 就绪回执失败不影响界面：宿主侧另有超时兜底，材质按无材质处理。
    applyMaterial('solid');
    return 'solid';
  }
}

/**
 * 窗口动态表面：焦点状态机与最大化事件。
 *
 * 焦点读的是窗口自身的 focus / blur，落点在侧栏容器上；最大化读的是宿主推的事件。
 * 两者都不改动 `:root`，因此长正文不会因为窗口焦点变化被标脏。
 *
 * `rebindKey` 是「侧栏容器可能被整棵换掉」的提示：侧栏在展开态与图标轨之间切换时，
 * `.app-shell-sidebar` 会重新挂载，旧节点上的属性随卸载消失。传入会变的值即可
 * 让这一层重新抓节点并补上当前状态，不必再在每个视图里各挂一遍。
 */
export function useWindowDynamicSurfaces(rebindKey?: unknown): void {
  const [focused, setFocused] = useState(true);
  const [maximized, setMaximized] = useState(false);

  // 焦点与最大化的当前值先落在 React 状态里，节点重建之后由下面的 effect 补齐。
  useEffect(() => {
    const onFocus = () => setFocused(true);
    const onBlur = () => setFocused(false);
    setFocused(true);
    window.addEventListener('focus', onFocus);
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('blur', onBlur);
    };
  }, []);

  useEffect(() => {
    if (!isDesktopHost()) return undefined;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void import('@tauri-apps/api/event')
      .then(({ listen }) => listen<boolean>(MAXIMIZED_EVENT, (event) => setMaximized(event.payload === true)))
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // 每次重建（含侧栏换形态）都把当前状态重新写到容器上。
  useEffect(() => {
    const sidebar = document.querySelector(APP_SHELL_SIDEBAR);
    if (sidebar === null) return;
    sidebar.setAttribute('data-window-focused', String(focused));
    sidebar.setAttribute('data-window-maximized', String(maximized));
  }, [focused, maximized, rebindKey]);
}

/**
 * 界面当前是否处于「材质可透光」状态。
 *
 * 只在设置页之类的说明性文案里用到：它读的是根节点上的结论，
 * 因此与 CSS 的判定必然一致，不会出现文案说半透、画面是实色的分叉。
 */
export function useWindowMaterial(): WindowMaterial {
  const [material, setMaterial] = useState<WindowMaterial>(() => {
    const value = document.documentElement.dataset.material;
    return value === 'mica' || value === 'vibrancy' ? value : 'solid';
  });
  useEffect(() => {
    const observer = new MutationObserver(() => {
      const value = document.documentElement.dataset.material;
      setMaterial(value === 'mica' || value === 'vibrancy' ? value : 'solid');
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-material'],
    });
    return () => observer.disconnect();
  }, []);
  return material;
}
