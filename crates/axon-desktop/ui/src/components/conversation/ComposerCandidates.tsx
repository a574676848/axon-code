import React from 'react';
import { createPortal } from 'react-dom';
import { Z_MENU } from './chatChrome';
import { MENU_ANCHOR_GAP, MENU_PANEL_ATTACHED_CLASS, MenuRow } from './MenuPanel';

/*
 * 输入坞的候选菜单：斜杠指令与 `@` 文件引用共用同一种形态。
 *
 * 菜单是贴着输入坞卡片上沿的浮层：与卡片同宽、间距 4px、向上展开。展开与否都不改
 * 卡片的版面（列表排进卡片时，一弹出输入框就被顶下去，长度还随候选条数变化）。
 *
 * 行与卡片几何全部来自 MenuPanel：32px 行、13/20 正文、8px 圆角、4px 内边距。
 * 高亮只有一份：键盘移动与指针移动写的是同一个下标，所以不会出现「指针停在一行、
 * 键盘选着另一行」。指针按下即采纳（不等到 click），并在按下时阻止默认行为，
 * 输入框于是始终握着焦点，采纳完可以直接接着打字。
 *
 * 几何走 portal 加 fixed 定位：正文与输入坞的祖先里有 overflow 裁剪，留在原地会被
 * 裁掉；与模型选择下拉、悬浮卡、右键菜单同一套做法。
 */

/** 菜单与窗口上沿的最小留白。 */
const VIEWPORT_MARGIN = 12;
/** 菜单高度上限：再长就在菜单内部滚动。 */
const MAX_HEIGHT = 400;
/** 高度下限：贴着窗口上沿时也留得出两行，不至于挤成一条缝。 */
const MIN_HEIGHT = 80;

export interface ComposerCandidate {
  /** 稳定标识，用于 React key。 */
  id: string;
  /** 主文本，指令名或路径。 */
  label: string;
  /** 右侧弱化说明；没有时不占位。 */
  hint?: string;
}

interface ComposerCandidateMenuProps {
  /** 列表元素 id：输入框用它声明 aria-controls 与 aria-activedescendant。 */
  id: string;
  /** 锚点（输入坞卡片）；为 null 时不渲染。 */
  anchor: HTMLElement | null;
  /** 无障碍名称，例如「斜杠指令」或「文件引用」。 */
  label: string;
  items: ComposerCandidate[];
  activeIndex: number;
  /** 悬浮或键盘移动到某一项。 */
  onActivate: (index: number) => void;
  /** 采纳某一项。 */
  onAccept: (index: number) => void;
  /** 指针落在菜单与卡片之外。 */
  onDismiss: () => void;
}

/** 菜单落点：与卡片同宽，贴着它上沿向上展开。 */
interface MenuGeometry {
  left: number;
  width: number;
  /** 与视口下沿的距离，也就是菜单自身底边的位置。 */
  bottom: number;
  maxHeight: number;
}

/** 一条候选的元素 id；输入框的 aria-activedescendant 指向它。 */
export function composerOptionId(listId: string, index: number): string {
  return `${listId}-option-${index}`;
}

export const ComposerCandidateMenu: React.FC<ComposerCandidateMenuProps> = ({
  id,
  anchor,
  label,
  items,
  activeIndex,
  onActivate,
  onAccept,
  onDismiss,
}) => {
  const menuRef = React.useRef<HTMLDivElement>(null);
  const viewportRef = React.useRef<HTMLDivElement>(null);
  const [geometry, setGeometry] = React.useState<MenuGeometry | null>(null);

  /*
   * 先量后摆：卡片会随输入长高，状态行之类的兄弟节点也会出现或消失（卡片整体上下移动），
   * 因此量的是卡片外那一层容器，它是这些变化共同撑起来的盒子；窗口尺寸单独再听一次。
   * 量到之前不渲染，免得先闪一个错位的框再弹回去。
   */
  React.useLayoutEffect(() => {
    if (anchor === null) return undefined;
    const measure = () => {
      const rect = anchor.getBoundingClientRect();
      const available = rect.top - MENU_ANCHOR_GAP - VIEWPORT_MARGIN;
      setGeometry({
        left: rect.left,
        width: rect.width,
        bottom: window.innerHeight - rect.top + MENU_ANCHOR_GAP,
        maxHeight: Math.max(MIN_HEIGHT, Math.min(MAX_HEIGHT, available)),
      });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(anchor.parentElement ?? anchor);
    window.addEventListener('resize', measure);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', measure);
    };
  }, [anchor]);

  // 指针落在菜单与卡片之外就收起：点输入框、点卡片底部的按钮都还算在卡片里。
  React.useEffect(() => {
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (menuRef.current?.contains(target) === true) return;
      if (anchor?.contains(target) === true) return;
      onDismiss();
    };
    document.addEventListener('pointerdown', handlePointerDown, true);
    return () => document.removeEventListener('pointerdown', handlePointerDown, true);
  }, [anchor, onDismiss]);

  /*
   * 让当前选中项始终留在可视范围内。
   *
   * 只在必要时最小滚动自己的容器（不用 scrollIntoView：它是全局滚动指令，
   * 还会带出动画）。键盘翻到列表末尾时，选中项会滚出容器，看不到选中了什么。
   * 位置一律相对滚动区量：两个节点的定位祖先相同，相减即这一段在滚动区里的位置。
   */
  React.useEffect(() => {
    const viewport = viewportRef.current;
    if (viewport === null) return;
    const active = viewport.querySelector<HTMLElement>('[aria-selected="true"]');
    if (active === null) return;
    const top = active.offsetTop - viewport.offsetTop;
    const bottom = top + active.offsetHeight;
    if (top < viewport.scrollTop) {
      viewport.scrollTop = top;
    } else if (bottom > viewport.scrollTop + viewport.clientHeight) {
      viewport.scrollTop = bottom - viewport.clientHeight;
    }
  }, [activeIndex, items.length]);

  if (geometry === null) return null;
  return createPortal(
    <div
      ref={menuRef}
      style={{
        left: geometry.left,
        width: geometry.width,
        bottom: geometry.bottom,
        maxHeight: geometry.maxHeight,
      }}
      className={`fixed ${Z_MENU} ${MENU_PANEL_ATTACHED_CLASS}`}
    >
      {/*
        长列表要看得见「下面还有内容」，因此这里把滑块显出来；
        圆角卡片自己裁掉四角，滚动条不会画到卡片外面。
      */}
      <div
        ref={viewportRef}
        id={id}
        role="listbox"
        aria-label={label}
        className="flex min-h-0 flex-col overflow-y-auto scroll-visible"
      >
        {items.map((item, index) => (
          <MenuRow
            key={item.id}
            id={composerOptionId(id, index)}
            role="option"
            ariaSelected={index === activeIndex}
            active={index === activeIndex}
            label={item.label}
            hint={item.hint}
            // 指针按下即采纳：不等到 click，输入框也不会在这一下里失焦。
            onMouseDown={(event) => {
              event.preventDefault();
              onAccept(index);
            }}
            // 用 mousemove 而不是 mouseenter：真的动了指针才换高亮，
            // 键盘滚动时停在原地的指针不会把选中项抢回去。
            onMouseMove={index === activeIndex ? undefined : () => onActivate(index)}
          />
        ))}
      </div>
    </div>,
    document.body,
  );
};
