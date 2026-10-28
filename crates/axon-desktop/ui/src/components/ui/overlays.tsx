import React, { useCallback, useContext, useEffect, useRef, useState } from 'react';
import { CheckIcon, ChevronDownIcon, CloseIcon } from '../../icons';
import { Button, IconButton } from './controls';
import { createPortal } from 'react-dom';
/*
 * 浮层基元：模态壳、单选下拉、气泡菜单。
 * 三者共享同一套卡片外观（白底、圆角、半像素描边加柔光）与同一套
 * 条目状态（悬浮浅灰底、选中浅灰底加对勾），保证全站观感一致。
 */

/** 卡片外观：白底、8px 圆角、描边加柔光；显式声明可见性，避免被行内隐藏容器连带隐藏。 */
const CARD =
  'visible bg-surface rounded-sm shadow-menu hairline border-border-hairline';

/**
 * 模态嵌套深度。
 *
 * 用 context 而不是查询 DOM：查询会把「当前这个弹窗」自己也数进去，
 * 弹窗任意一次重渲染都会误判为嵌套并把遮罩摘掉。context 只统计祖先，
 * 子弹窗读到 1，父弹窗读到 0，判定稳定。
 */
const ModalDepthContext = React.createContext(0);

/** 条目外观：13px 文本、8px 圆角、悬浮与选中都是同一档浅灰底。 */
const ITEM =
  'w-full flex items-center gap-2 px-2.5 h-8 rounded-xs text-row text-left transition-colors duration-100 cursor-pointer select-none';

/** 点击组件外部时触发回调。 */
/**
 * 点击外部关闭。
 *
 * `containsExtra` 让调用方声明「除了触发区，还有哪些节点算内部」：菜单是挂在
 * document.body 上的浮层，不在触发区的 DOM 子树里，只认触发区会把「点菜单里的
 * 按钮」判成点外部，mousedown 先关掉菜单、click 落空，菜单里的每一项就都点不动了。
 */
function useDismiss(
  active: boolean,
  onDismiss: () => void,
  containsExtra?: (target: Node) => boolean,
) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!active) return;
    const handlePointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      const insideTrigger = ref.current !== null && ref.current.contains(target);
      const insideExtra = containsExtra?.(target) ?? false;
      if (!insideTrigger && !insideExtra) {
        onDismiss();
      }
    };
    document.addEventListener('mousedown', handlePointerDown);
    return () => document.removeEventListener('mousedown', handlePointerDown);
  }, [active, onDismiss, containsExtra]);
  return ref;
}

export interface MenuLabelProps {
  children: React.ReactNode;
}

export const MenuLabel: React.FC<MenuLabelProps> = ({ children }) => (
  <div className="px-2 pt-2 pb-1 text-code leading-4 font-medium text-text-tertiary">{children}</div>
);

export const MenuSeparator: React.FC = () => (
  <div className="my-1 h-hairline bg-border-hairline" role="separator" />
);

export interface MenuItemProps {
  /** 是否为当前选中项。 */
  selected?: boolean;
  /** 选中回调。 */
  onSelect: () => void;
  /** 前置图标。 */
  icon?: React.ReactNode;
  /** 右侧快捷键或附加文本。 */
  hint?: string;
  /** 危险操作样式。 */
  danger?: boolean;
  children: React.ReactNode;
}

export const MenuItem: React.FC<MenuItemProps> = ({
  selected = false,
  onSelect,
  icon,
  hint,
  danger = false,
  children,
}) => (
  <button
    type="button"
    role="menuitem"
    onClick={onSelect}
    className={`${ITEM} ${selected ? 'bg-active font-medium' : 'hover:bg-hover'} ${
      danger ? 'text-danger' : 'text-text-primary'
    }`}
  >
    {icon !== undefined && <span className="shrink-0 flex text-text-secondary">{icon}</span>}
    <span className="flex-1 min-w-0 truncate">{children}</span>
    {hint !== undefined && (
      <span className="shrink-0 text-code text-text-tertiary font-mono">{hint}</span>
    )}
    {selected && <CheckIcon size={13} className="shrink-0 text-text-link" />}
  </button>
);

export interface DropdownProps {
  /** 触发区渲染函数，拿到展开状态与切换回调。 */
  trigger: (state: { open: boolean; toggle: () => void }) => React.ReactNode;
  /** 菜单内容。 */
  children: React.ReactNode;
  /** 对齐方式。 */
  align?: 'start' | 'end';
  /** 菜单最小宽度类名。 */
  menuClassName?: string;
  /**
   * 整块卡片的外观；缺省用全站统一的白底描边卡片。
   *
   * 只有圆角或材质需要按浮层档位走时（例如对齐既有工作台产品的 16px 浮层圆角）才覆盖，
   * 覆盖值必须仍然是完整的一套外观类，否则会得到一个没有底色或没有柔光的浮层。
   */
  surfaceClassName?: string;
  /** 触发区容器附加类名，仅用于布局（例如让触发区在窄列里可被压缩）。 */
  className?: string;
  /** 浮层语义角色：菜单用 menu，读数明细这类内容用 dialog。 */
  panelRole?: 'menu' | 'dialog';
  /** 展开状态变化回调，供外层保持触发区可见。 */
  onOpenChange?: (open: boolean) => void;
}

export const Dropdown: React.FC<DropdownProps> = ({
  trigger,
  children,
  align = 'start',
  menuClassName = 'min-w-[200px]',
  surfaceClassName = CARD,
  className = '',
  panelRole = 'menu',
  onOpenChange,
}) => {
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const menuRef = useRef<HTMLDivElement>(null);
  // 菜单本身也算内部节点，否则点菜单里的按钮会被判成点外部而先关闭。
  const insideMenu = useCallback(
    (target: Node) => menuRef.current?.contains(target) ?? false,
    [],
  );
  const ref = useDismiss(open, close, insideMenu);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);

  useEffect(() => {
    onOpenChange?.(open);
  }, [open, onOpenChange]);

  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        close();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [open, close]);

  // 菜单挂到 body 并以固定定位摆放：锚点间距 4px，按锚点左右边对齐，
  // 再以 12px 边距夹取到窗口内。夹取基准是窗口而不是侧栏，
  // 因此菜单可以越过侧栏边缘，同时永远不会溢出窗口或被侧栏裁掉。
  useEffect(() => {
    if (!open) {
      setPosition(null);
      return;
    }
    const place = () => {
      const anchorRect = ref.current?.getBoundingClientRect();
      const menu = menuRef.current;
      if (anchorRect === undefined || menu === null) return;
      const margin = 12;
      const menuWidth = menu.offsetWidth;
      const menuHeight = menu.offsetHeight;
      let left = align === 'end' ? anchorRect.right - menuWidth : anchorRect.left;
      let top = anchorRect.bottom + 4;
      if (menuWidth > 0) {
        left = Math.min(Math.max(left, margin), window.innerWidth - menuWidth - margin);
      }
      if (menuHeight > 0) {
        top = Math.min(Math.max(top, margin), window.innerHeight - menuHeight - margin);
      }
      setPosition((current) =>
        current?.left === left && current.top === top ? current : { left, top },
      );
    };
    place();
    window.addEventListener('resize', place);
    // 滚动时关闭而不是跟随重排：契约禁止 scroll 监听，而菜单挂在 body 上，
    // 跟随重排本就需要滚动事件。关闭更简单，也不会让菜单与锚点脱节。
    // 例外是菜单内部自己的滚动：作业列表与输出区本来就要滚，滚它们不该关掉菜单。
    const closeOnWheel = (event: WheelEvent) => {
      const target = event.target;
      if (target instanceof Node && menuRef.current?.contains(target) === true) return;
      close();
    };
    window.addEventListener('wheel', closeOnWheel, { passive: true });
    return () => {
      window.removeEventListener('resize', place);
      window.removeEventListener('wheel', closeOnWheel);
    };
  }, [open, align, ref, close]);

  return (
    <div className={`relative inline-flex ${className}`} ref={ref}>
      {trigger({ open, toggle: () => setOpen((value) => !value) })}
      {open &&
        createPortal(
          <div
            ref={menuRef}
            role={panelRole}
            style={{
              position: 'fixed',
              left: position?.left ?? 0,
              top: position?.top ?? 0,
              visibility: position === null ? 'hidden' : 'visible',
            }}
            className={`z-[1100] p-1 ${surfaceClassName} ${menuClassName}`}
          >
            {children}
          </div>,
          document.body,
        )}
    </div>
  );
};

export interface SelectOption {
  /** 选项值。 */
  value: string;
  /** 选项显示文本。 */
  label: string;
  /** 是否不可选；不可选项仍然列出，并说明原因。 */
  disabled?: boolean;
  /** 选项右侧的补充说明（例如「当前不可用」）。 */
  hint?: string;
}

export interface SelectProps {
  /** 当前值。 */
  value: string;
  /** 候选项。 */
  options: SelectOption[];
  /** 选中回调。 */
  onChange: (value: string) => void;
  /** 无障碍名称。 */
  label: string;
  /** 禁用态。 */
  disabled?: boolean;
  /** 控件宽度类名。 */
  className?: string;
}

export const Select: React.FC<SelectProps> = ({
  value,
  options,
  onChange,
  label,
  disabled = false,
  className = '',
}) => {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  /*
   * 菜单挂到 body 并以固定定位摆放。
   *
   * 设置弹窗的外壳是 `overflow-clip`，绝对定位的下拉会在面板底缘被裁掉；
   * 挂到 body 后夹取基准变成窗口，菜单永远完整可见。
   */
  const [placement, setPlacement] = useState<{
    left: number;
    top: number;
    width: number;
    above: boolean;
    maxHeight: number;
  } | null>(null);
  const close = useCallback(() => setOpen(false), []);
  const ref = useDismiss(open, close, (target) => menuRef.current?.contains(target) ?? false);

  const currentIndex = options.findIndex((option) => option.value === value);
  const current = currentIndex >= 0 ? options[currentIndex] : undefined;

  useEffect(() => {
    if (open) {
      setActiveIndex(currentIndex >= 0 ? currentIndex : 0);
    }
  }, [open, currentIndex]);

  // 窗口尺寸或滚动位置变化时收起：固定定位的菜单不跟随锚点。
  useEffect(() => {
    if (!open) return;
    const dismiss = () => setOpen(false);
    const handleWheel = (event: WheelEvent) => {
      // 菜单自身要滚：只有滚轮落在菜单外时才收起。
      if (menuRef.current?.contains(event.target as Node) === true) return;
      setOpen(false);
    };
    window.addEventListener('resize', dismiss);
    window.addEventListener('wheel', handleWheel, { passive: true });
    return () => {
      window.removeEventListener('resize', dismiss);
      window.removeEventListener('wheel', handleWheel);
    };
  }, [open]);

  const openMenu = () => {
    const rect = buttonRef.current?.getBoundingClientRect();
    if (rect === undefined) return;
    const below = window.innerHeight - rect.bottom - 16;
    const above = rect.top - 16;
    // 下方不够且上方更宽裕时向上展开；菜单高度上限 256，装不下就在内部滚。
    const useAbove = below < 220 && above > below;
    const width = Math.max(rect.width, 160);
    const left = Math.min(Math.max(rect.left, 12), Math.max(12, window.innerWidth - width - 12));
    setPlacement({
      left,
      top: useAbove ? rect.top - 4 : rect.bottom + 4,
      width,
      above: useAbove,
      maxHeight: Math.max(120, Math.min(256, useAbove ? above : below)),
    });
    setOpen(true);
  };

  const commit = (index: number) => {
    const option = options[index];
    if (option !== undefined && option.disabled !== true) {
      onChange(option.value);
    }
    setOpen(false);
  };

  /** 方向键只在可选之间移动：停用项不该被键盘选中。 */
  const move = (delta: number): void => {
    if (options.length === 0) return;
    let next = activeIndex;
    for (let step = 0; step < options.length; step += 1) {
      next = (next + delta + options.length) % options.length;
      if (options[next]?.disabled !== true) {
        setActiveIndex(next);
        return;
      }
    }
  };

  /**
   * 触发器与选项都禁止冒泡默认行为。
   *
   * 组件常被放进 `<label>` 形态的字段容器里，而 label 的激活行为会把点击转发回
   * 触发按钮：选择后下拉会立刻重新弹开并盖住下方字段，用户点击字段时会被浮层
   * 截获、静默改掉已选值。这里阻断默认行为，选择即关闭。
   */
  const stopLabelActivation = (event: React.MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (disabled) return;
    if (!open && (event.key === 'Enter' || event.key === ' ' || event.key === 'ArrowDown')) {
      event.preventDefault();
      setOpen(true);
      return;
    }
    if (!open) return;
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      move(1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      move(-1);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      commit(activeIndex);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      setOpen(false);
    }
  };

  return (
    <div className={`relative ${className}`} ref={ref}>
      <button
        ref={buttonRef}
        type="button"
        aria-label={label}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={(event) => {
          stopLabelActivation(event);
          if (open) {
            setOpen(false);
          } else {
            openMenu();
          }
        }}
        onKeyDown={handleKeyDown}
        className={`w-full h-8 flex items-center justify-between gap-2 px-2 rounded-sm bg-surface hairline text-row text-text-primary transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-50 ${
          open ? 'border-focus' : 'border-border-input hover:border-border-strong'
        }`}
      >
        <span className="truncate">{current?.label ?? value}</span>
        <ChevronDownIcon
          size={14}
          className={`shrink-0 text-text-tertiary transition-transform duration-150 ${
            open ? 'rotate-180' : ''
          }`}
        />
      </button>

      {open &&
        placement !== null &&
        createPortal(
          <div
            ref={menuRef}
            role="listbox"
            aria-label={label}
            className={`fixed z-[1100] p-1 overflow-y-auto scroll-visible ${CARD}`}
            style={{
              left: placement.left,
              top: placement.top,
              width: placement.width,
              maxHeight: placement.maxHeight,
              transform: placement.above ? 'translateY(-100%)' : undefined,
            }}
          >
            {options.length === 0 ? (
              <div className="px-2 py-1.5 text-row leading-4 text-text-tertiary">暂无可选项</div>
            ) : (
              options.map((option, index) => (
                <button
                  key={option.value}
                  type="button"
                  role="option"
                  aria-selected={option.value === value}
                  aria-disabled={option.disabled === true ? true : undefined}
                  disabled={option.disabled === true}
                  onMouseEnter={() => {
                    if (option.disabled !== true) setActiveIndex(index);
                  }}
                  onClick={(event) => {
                    stopLabelActivation(event);
                    commit(index);
                  }}
                  className={`${ITEM} ${
                    option.value === value ? 'bg-active font-medium' : 'hover:bg-hover'
                  } ${index === activeIndex && option.value !== value ? 'bg-hover' : ''} ${
                    option.disabled === true ? 'cursor-not-allowed opacity-40' : ''
                  }`}
                >
                  <span className="flex-1 min-w-0 truncate">{option.label}</span>
                  {option.hint !== undefined && (
                    <span className="shrink-0 text-code text-text-tertiary">{option.hint}</span>
                  )}
                  {option.value === value && (
                    <CheckIcon size={13} className="shrink-0 text-text-link" />
                  )}
                </button>
              ))
            )}
          </div>,
          document.body,
        )}
    </div>
  );
};

export interface ModalProps {
  /** 是否打开。 */
  open: boolean;
  /** 关闭回调。 */
  onClose: () => void;
  /** 标题。 */
  title: string;
  /** 标题右侧操作区。 */
  actions?: React.ReactNode;
  /** 顶部形态：独立标题栏、右上角浮动关闭，或无内置头部由子面板自行排布。 */
  variant?: 'bar' | 'floating' | 'headless';
  /** 面板尺寸类名。 */
  panelClassName?: string;
  /** 内容。 */
  children: React.ReactNode;
}

export const Modal: React.FC<ModalProps> = ({
  open,
  onClose,
  title,
  actions,
  variant = 'bar',
  panelClassName = 'w-full max-w-4xl h-[640px]',
  children,
}) => {
  const dialogRef = useRef<HTMLDivElement>(null);
  /** 祖先弹窗深度：0 表示这是最外层，需要画遮罩。 */
  const depth = useContext(ModalDepthContext);
  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        const dialogs = document.querySelectorAll('[role="dialog"][aria-modal="true"]');
        if (dialogs.item(dialogs.length - 1) !== dialogRef.current) return;
        event.preventDefault();
        onClose();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [open, onClose]);

  if (!open) return null;

  /*
   * 嵌套弹窗不再叠第二层遮罩：父弹窗已经把背景压暗，
   * 再叠一层会让整窗变成近半黑并两次模糊。深度来自 context，只统计祖先，
   * 因此当前弹窗自己的重渲染不会把遮罩摘掉。
   */
  const nested = depth > 0;

  return (
    <ModalDepthContext.Provider value={depth + 1}>
      <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4 sm:p-6">
      {!nested && (
        <div
          className="overlay-scrim-in absolute inset-0 bg-scrim backdrop-blur-[2px]"
          onClick={onClose}
          aria-hidden="true"
        />
      )}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        /*
         * `max-h-full` 是硬性约束：外层是 `fixed inset-0` 加内边距的居中容器，
         * 没有它时面板会按内容长高、顶出上下边界（标题被裁掉、列表既看不全也滚不动），
         * 内层的 `flex-1 min-h-0` 也就没有可收缩的高度可用。
         *
         * `overflow-clip` 而不是 `overflow-hidden`：`hidden` 仍然是一个可滚动容器，
         * 键盘把焦点移到弹窗内部控件时，浏览器会为「把焦点滚进视野」而滚动它，
         * 整块内容被顶出面板、底栏下方留出空白（内容越多偏得越远）。
         * `clip` 让面板彻底不是滚动容器，浏览器再也没有可滚动的对象。
         */
        className={`overlay-panel-in relative flex flex-col overflow-clip bg-surface rounded-lg shadow-modal hairline border-border-hairline max-h-full min-h-0 ${panelClassName}`}
      >
        {variant === 'bar' && (
          <div className="shrink-0 px-5 py-3 flex items-center justify-between gap-4 hairline-b border-border-hairline">
            <h2 className="text-body font-semibold text-text-primary">{title}</h2>
            <div className="shrink-0 flex items-center gap-1.5">
              {actions}
              <IconButton label="关闭" onClick={onClose} className="ml-0.5">
                <CloseIcon size={14} />
              </IconButton>
            </div>
          </div>
        )}
        {variant === 'floating' && (
          <div className="absolute top-3 right-3 z-20 flex items-center gap-2">
            {actions}
            <IconButton label="关闭" onClick={onClose}>
              <CloseIcon size={14} />
            </IconButton>
          </div>
        )}
        <div className="flex-1 min-h-0 flex overflow-clip">{children}</div>
      </div>
      </div>
    </ModalDepthContext.Provider>
  );
};

/** 对话框宽度档位。 */
export type DialogSize = 'sm' | 'md' | 'lg';

const DIALOG_SIZE: Record<DialogSize, string> = {
  sm: 'w-full max-w-md',
  md: 'w-full max-w-lg',
  lg: 'w-full max-w-2xl',
};

/** 右下角主操作。 */
export interface DialogAction {
  /** 按钮文案。 */
  label: string;
  /** 忙碌时的替代文案。 */
  busyLabel?: string;
  /** 前置图标。 */
  icon?: React.ReactNode;
  /** 是否忙碌。 */
  busy?: boolean;
  /** 是否禁用。 */
  disabled?: boolean;
  /** 点击回调。 */
  onClick: () => void;
}

export interface DialogProps {
  /** 是否打开。 */
  open: boolean;
  /** 关闭回调。 */
  onClose: () => void;
  /** 标题。 */
  title: string;
  /** 宽度档位，默认中号。 */
  size?: DialogSize;
  /** 右下角主操作。 */
  primary?: DialogAction;
  /** 主操作左侧的次操作，默认描边样式。 */
  secondary?: DialogAction & { danger?: boolean };
  /** 底部左侧的一行说明。 */
  hint?: string;
  /** 正文。 */
  children: React.ReactNode;
}

/**
 * 统一的对话框形态：关闭只走右上角图标，主操作固定右下角。
 *
 * 所有弹窗都必须经由这里拼装，不允许各自摆放按钮位置，
 * 以保证后续新增弹窗与既有弹窗的交互完全一致。
 */
export const Dialog: React.FC<DialogProps> = ({
  open,
  onClose,
  title,
  size = 'md',
  primary,
  secondary,
  hint,
  children,
}) => {
  const hasFooter = primary !== undefined || secondary !== undefined || hint !== undefined;
  return (
    <Modal open={open} onClose={onClose} title={title} panelClassName={DIALOG_SIZE[size]}>
      <div className="w-full min-h-0 flex flex-col">
        <div className="flex-1 min-h-0 overflow-y-auto px-5 py-4 space-y-3.5">{children}</div>
        {hasFooter && (
          <div className="shrink-0 flex items-center gap-2 px-5 py-3 bg-surface-sunken/30 hairline-t border-border-hairline">
            {hint !== undefined && (
              <span className="flex-1 min-w-0 truncate text-xs text-text-tertiary" title={hint}>
                {hint}
              </span>
            )}
            <div className="ml-auto shrink-0 flex items-center gap-2">
              {secondary !== undefined && (
                <Button
                  variant={secondary.danger === true ? 'danger' : 'outline'}
                  size="md"
                  disabled={secondary.disabled === true || secondary.busy === true}
                  onClick={secondary.onClick}
                >
                  {secondary.icon}
                  {secondary.busy === true && secondary.busyLabel !== undefined
                    ? secondary.busyLabel
                    : secondary.label}
                </Button>
              )}
              {primary !== undefined && (
                <Button
                  variant="primary"
                  size="md"
                  disabled={primary.disabled === true || primary.busy === true}
                  onClick={primary.onClick}
                >
                  {primary.icon}
                  {primary.busy === true && primary.busyLabel !== undefined
                    ? primary.busyLabel
                    : primary.label}
                </Button>
              )}
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
};
