import React, { useEffect } from 'react';

/*
 * 弹层的焦点纪律：进入时把焦点移进面板，Tab 与 Shift + Tab 不越出面板，关闭后归还焦点。
 *
 * 全站只有这一份实现：模态、对话框与命令面板的焦点规则完全一样，差别只在面板本身。
 * 没有它的时候，打开设置后 Tab 会继续走到底下那一层真实页面里（遮罩只是挡住了眼睛，
 * 没有挡住键盘），关闭后焦点又掉回文档开头，键盘用户每开一次弹窗就要重新找位置。
 */

/** 浏览器认可的可聚焦元素；顺序与焦点自然遍历顺序一致。 */
const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  'summary',
  '[contenteditable="true"]',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

/**
 * 容器内此刻真的能落焦点的元素。
 *
 * 只看布局盒：`display: none` 与整块收起的区域量不到盒子，因此不会把焦点送到
 * 用户看不见的地方（弹窗里常有「仅某种状态才出现」的表单行）。
 *
 * 传 `selector` 可以把范围收窄到某一类目标：菜单只在自己的 `[role="menuitem"]`
 * 之间移动，弹窗则按整块可聚焦集合遍历。两种用法共用这一份可见性判定。
 */
export function focusableElements(
  container: HTMLElement | null,
  selector: string = FOCUSABLE,
): HTMLElement[] {
  if (container === null) return [];
  return [...container.querySelectorAll<HTMLElement>(selector)].filter(
    (element) =>
      element.getAttribute('aria-hidden') !== 'true' &&
      (element.offsetWidth > 0 || element.offsetHeight > 0 || element.getClientRects().length > 0),
  );
}

/**
 * 当前最上层的模态。
 *
 * 嵌套弹窗里两层都会挂这条规则，只有最上层接住 Tab：否则父层会把焦点从子弹窗
 * 里拽回自己身上，人在子层里按 Tab 就看到焦点来回跳。
 */
function topmostDialog(): Element | null {
  const dialogs = document.querySelectorAll('[role="dialog"][aria-modal="true"]');
  const last = dialogs.length - 1;
  return last < 0 ? null : dialogs.item(last);
}

/**
 * 装配焦点纪律。
 *
 * @param containerRef 面板节点；未挂载时为空。
 * @param active 弹层是否处于打开态。关闭的那一帧归还焦点。
 */
export function useFocusTrap(
  containerRef: React.RefObject<HTMLElement | null>,
  active: boolean,
): void {
  useEffect(() => {
    const container = containerRef.current;
    if (!active || container === null) return undefined;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;

    /*
     * 焦点移入放在下一帧：面板里的输入框自己声明了 `autoFocus` 时，
     * 那一份意图先落地，这里只补上「里面还没有任何落点」的情况，不抢它的焦点。
     */
    const moveIn = (): void => {
      if (container.contains(document.activeElement)) return;
      // 面板自身带 `tabIndex=-1`，因此纯说明型弹窗（没有一个可聚焦元素）也有落点。
      const first = focusableElements(container)[0] ?? container;
      first.focus({ preventScroll: true });
    };
    const frame = window.requestAnimationFrame(moveIn);

    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Tab') return;
      if (topmostDialog() !== container) return;
      const items = focusableElements(container);
      if (items.length === 0) {
        event.preventDefault();
        container.focus({ preventScroll: true });
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const current = document.activeElement;
      if (current === null || !container.contains(current)) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
        return;
      }
      if (event.shiftKey && (current === first || current === container)) {
        event.preventDefault();
        last.focus();
        return;
      }
      if (!event.shiftKey && current === last) {
        event.preventDefault();
        first.focus();
      }
    };

    // 捕获阶段：面板内子控件自己的 Tab 处理不该把焦点送到面板之外。
    document.addEventListener('keydown', handleKeyDown, true);
    return () => {
      window.cancelAnimationFrame(frame);
      document.removeEventListener('keydown', handleKeyDown, true);
      const current = document.activeElement;
      const stillInside =
        current === null || current === document.body || container.contains(current);
      // 只在焦点仍被这次弹窗占着时归还：用户已经自己点走，就不该把焦点抢回来。
      if (stillInside && previous !== null && previous.isConnected) {
        previous.focus({ preventScroll: true });
      }
    };
  }, [active, containerRef]);
}
