import React from 'react';
import { createPortal } from 'react-dom';

/*
 * 统一右键菜单。
 *
 * 卡片按设计系统 §4 与 §10.1 取值：圆角 r-sm（8px）、菜单级阴影、夹取边距 12px、
 * 关闭条件为点击外部、Esc 与滚轮；行内度量（34px 行高、6px/8px 内边距、13px 字号、
 * 11px 快捷键、0.5px 分组线）沿用工作台既有下拉的同一套数值，因此它与模型选择、
 * 会话操作那些下拉读起来是同一套东西。行圆角取 4px：同心律要求内层等于外层减内边距
 * （8px 减 4px）。
 *
 * 三处刻意的实现选择：
 * 1. 走 portal 渲染到 body：正文与输入坞的祖先里有 overflow 裁剪与 transform，
 *    留在原地会被裁掉，或被当成 fixed 定位的基准，菜单就跑偏了。
 * 2. 先量后摆：首帧量不到尺寸，直接按光标落点摆会在贴右下边缘时先闪一下再弹回。
 * 3. Esc 在捕获阶段就地吞掉：菜单打开时这一键属于菜单，不能穿透到底层的全局快捷键
 *    （会话运行中那就是「中断本轮」）。
 *
 * 层级取所有浮层之上（模态 1000、下拉 1100）：菜单是全局机制，弹窗与浮层里的
 * 正文同样要能右键，压在模态之下等于那些位置没有菜单。
 */

/** 一个菜单项。 */
export interface ContextMenuItem {
  /** 菜单文字。 */
  label: string;
  /** 点击执行的动作者。 */
  action: () => void;
  /** 不可用时置灰并保持占位。 */
  disabled?: boolean;
  /** 右侧快捷键提示；只作提示，按键本身由各自组件处理。 */
  shortcut?: string;
  /** 在这一项之前画一条分组线。 */
  separatorBefore?: boolean;
}

/** 菜单当前的落点与内容。 */
export interface ContextMenuState {
  items: ContextMenuItem[];
  position: { x: number; y: number };
}

/** 菜单控制器：由 useContextMenu 提供。 */
export interface ContextMenuController {
  menu: ContextMenuState | null;
  /** 在 React 事件位置打开菜单，并阻止系统默认菜单。 */
  open: (event: React.MouseEvent, items: ContextMenuItem[]) => void;
  /** 在视口坐标打开菜单；文档级右键事件拿不到 React 合成事件，只能走这里。 */
  openAt: (x: number, y: number, items: ContextMenuItem[]) => void;
  close: () => void;
}

/** 菜单与窗口边缘的最小间距；设计系统 §10.1 规定水平与垂直都以 12px 夹取。 */
const EDGE_GAP = 12;

export const ContextMenu: React.FC<ContextMenuState & { onClose: () => void }> = ({
  items,
  position,
  onClose,
}) => {
  const menuRef = React.useRef<HTMLDivElement>(null);
  const [placed, setPlaced] = React.useState<{ left: number; top: number } | null>(null);
  const [active, setActive] = React.useState(-1);

  // 先量后摆：只在量到尺寸之后才显示，避免贴边时先闪一下再弹回。
  React.useLayoutEffect(() => {
    const menu = menuRef.current;
    if (menu === null) return;
    const rect = menu.getBoundingClientRect();
    setPlaced({
      left: Math.max(EDGE_GAP, Math.min(position.x, window.innerWidth - rect.width - EDGE_GAP)),
      top: Math.max(EDGE_GAP, Math.min(position.y, window.innerHeight - rect.height - EDGE_GAP)),
    });
  }, [position]);

  // 菜单是短暂的浮层：点到别处、滚轮滚动、窗口尺寸变化或失去焦点都应该收起。
  React.useEffect(() => {
    const handlePointerDown = (event: MouseEvent) => {
      if (menuRef.current?.contains(event.target as Node) !== true) onClose();
    };
    document.addEventListener('mousedown', handlePointerDown, true);
    // 滚动时关闭而不是跟随重排：跟随重排需要滚动驱动逻辑，那是动效铁律禁止的。
    window.addEventListener('wheel', onClose, { passive: true });
    window.addEventListener('resize', onClose);
    window.addEventListener('blur', onClose);
    return () => {
      document.removeEventListener('mousedown', handlePointerDown, true);
      window.removeEventListener('wheel', onClose);
      window.removeEventListener('resize', onClose);
      window.removeEventListener('blur', onClose);
    };
  }, [onClose]);

  /** 可落点的项下标；禁用项不参与键盘移动。 */
  const movable = React.useMemo(
    () => items.map((item, index) => (item.disabled === true ? -1 : index)).filter((i) => i >= 0),
    [items],
  );

  React.useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        return;
      }
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        if (movable.length === 0) return;
        event.preventDefault();
        event.stopPropagation();
        const step = event.key === 'ArrowDown' ? 1 : -1;
        const at = movable.indexOf(active);
        const next =
          at < 0 ? (step > 0 ? 0 : movable.length - 1) : (at + step + movable.length) % movable.length;
        setActive(movable[next]);
        return;
      }
      if (event.key === 'Enter' && active >= 0) {
        event.preventDefault();
        event.stopPropagation();
        const item = items[active];
        if (item !== undefined && item.disabled !== true) {
          item.action();
          onClose();
        }
      }
    };
    // 捕获阶段：菜单打开期间由菜单独占键盘，包括 Escape。
    document.addEventListener('keydown', handleKeyDown, true);
    return () => document.removeEventListener('keydown', handleKeyDown, true);
  }, [active, items, movable, onClose]);

  return createPortal(
    <div
      ref={menuRef}
      role="menu"
      style={{
        left: placed?.left ?? position.x,
        top: placed?.top ?? position.y,
        visibility: placed === null ? 'hidden' : 'visible',
      }}
      className="fixed z-[1200] min-w-[144px] max-w-[360px] max-h-[calc(100vh-24px)] overflow-y-auto p-1 rounded-sm bg-surface hairline border-border-hairline shadow-menu"
    >
      {items.map((item, index) => (
        <React.Fragment key={`${item.label}:${index}`}>
          {item.separatorBefore === true && (
            <div role="separator" className="h-hairline my-[3px] mx-0.5 bg-border-hairline" />
          )}
          <button
            type="button"
            role="menuitem"
            disabled={item.disabled}
            onMouseEnter={() => setActive(item.disabled === true ? -1 : index)}
            onClick={() => {
              if (item.disabled === true) return;
              item.action();
              onClose();
            }}
            className={`w-full min-h-[34px] px-2 py-1.5 flex items-center gap-1.5 rounded-xs text-left text-row leading-5 text-text-primary transition-colors duration-100 enabled:cursor-pointer enabled:hover:bg-hover disabled:opacity-60 disabled:cursor-not-allowed ${
              index === active ? 'bg-hover' : ''
            }`}
          >
            <span className="flex-1 min-w-0 truncate">{item.label}</span>
            {item.shortcut !== undefined && (
              <span className="shrink-0 ml-auto text-code text-text-tertiary">{item.shortcut}</span>
            )}
          </button>
        </React.Fragment>
      ))}
    </div>,
    document.body,
  );
};

/** 右键菜单控制器：管理菜单的开关与落点。 */
export function useContextMenu(): ContextMenuController {
  const [menu, setMenu] = React.useState<ContextMenuState | null>(null);

  const openAt = React.useCallback((x: number, y: number, items: ContextMenuItem[]) => {
    if (items.length === 0) return;
    setMenu({ items, position: { x, y } });
  }, []);

  const open = React.useCallback(
    (event: React.MouseEvent, items: ContextMenuItem[]) => {
      // 阻止系统默认菜单：这一步同时压掉 webview 自己的菜单，两者不能同时出现。
      // 阻止冒泡同样重要：文档级的那一份处理器据此让位，不会重复弹一次。
      event.preventDefault();
      event.stopPropagation();
      openAt(event.clientX, event.clientY, items);
    },
    [openAt],
  );

  const close = React.useCallback(() => setMenu(null), []);

  return { menu, open, openAt, close };
}
