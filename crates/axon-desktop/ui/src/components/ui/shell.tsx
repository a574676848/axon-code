import React from 'react';
import { IconNewChat } from '../../icons';
import { INTERACTIVE_ROW, Text } from './surfaces';
/*
 * 外壳级共享组件：侧边栏列容器与新建会话按钮。
 * 这两个外观在展开态与图标轨两处都要用，因此收敛到一处定义。
 */

export interface SidebarColumnProps {
  /** 无障碍名称。 */
  label: string;
  /** 展开态宽度；图标轨形态由 variant 决定，无需传值。 */
  width?: number;
  /** 形态：展开成列，或收成图标轨。 */
  variant?: 'expanded' | 'rail';
  /** 附加类名，仅用于布局。 */
  className?: string;
  children: React.ReactNode;
}

/** 侧边栏列：展开与收起共享同一托盘底色与层级。 */
export const SidebarColumn: React.FC<SidebarColumnProps> = ({
  label,
  width,
  variant = 'expanded',
  className = '',
  children,
}) => (
  <aside
    aria-label={label}
    style={variant === 'expanded' && width !== undefined ? { width } : undefined}
    className={`app-shell-sidebar relative flex flex-col shrink-0 h-full z-10 ${
      variant === 'rail' ? 'w-14 items-center py-2.5' : ''
    } ${className}`}
  >
    {children}
  </aside>
);

export interface NewSessionButtonProps {  /** 点击行为。 */
  onClick: () => void;
  /** 图标轨形态：只显示图标，并改用方形控件。 */
  compact?: boolean;
}

/** 新建会话按钮：展开态显示文案与悬浮才出现的快捷键提示。 */
export const NewSessionButton: React.FC<NewSessionButtonProps> = ({ onClick, compact = false }) => {
  if (compact) {
    return (
      <button
        type="button"
        onClick={onClick}
        aria-label="新会话"
        title="新会话 (Ctrl + Alt + N)"
        className={`w-9 h-9 flex items-center justify-center rounded-md hairline border-border-strong bg-surface text-text-primary shadow-btn-outline active:scale-[0.98] ${INTERACTIVE_ROW}`}
      >
        <IconNewChat size={16} />
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={onClick}
      className={`group relative w-full h-9 px-3 flex items-center justify-start rounded-md hairline border-border-strong bg-surface shadow-btn-outline active:scale-[0.99] ${INTERACTIVE_ROW}`}
    >
      {/* 图标与文案整体居中，快捷键提示绝对定位，出现时不会把文案挤偏 */}
      <span className="flex items-center gap-1.5 min-w-0">
        <IconNewChat size={15} className="shrink-0" />
        <Text strong truncate>
          新会话
        </Text>
      </span>
      <span className="absolute right-2.5 top-1/2 -translate-y-1/2 px-1.5 py-0.5 rounded-xs bg-surface-sunken hairline border-border font-mono text-[10px] leading-3 text-text-tertiary opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-focus-visible:opacity-100">
        Ctrl + Alt + N
      </span>
    </button>
  );
};

export interface NavGlideProps {
  /** 当前选中项在导航列里的序号；没有选中项时传 null，指示标淡出。 */
  activeIndex: number | null;
  /** 单行高度。 */
  itemSize: number;
  /** 行间距。 */
  gap: number;
  /** 指示标自身高度，默认 16px。 */
  barSize?: number;
  /** 相对列容器左缘的水平偏移（图标轨上按钮居中，需要缩进）。 */
  inset?: number;
}

/**
 * 导航滑动指示标：整列只有一枚 2px 主色胶囊，在选中项之间平移。
 *
 * 与「每个按钮各画一条」相比，滑动只走一次 `transform`：切换导航时只有这枚胶囊
 * 在动，行本身不重排，也不会出现两条指示条同时亮的中间帧。
 * 容器需要 `relative`，且从第一行开始摆放（不要把它放进带内边距的包装层，
 * 否则位移基准会跟着偏）。
 */
export const NavGlide: React.FC<NavGlideProps> = ({
  activeIndex,
  itemSize,
  gap,
  barSize = 16,
  inset = 0,
}) => (
  <span
    aria-hidden="true"
    className="pointer-events-none absolute top-0 w-0.5 rounded-full bg-primary"
    style={{
      left: inset,
      height: barSize,
      opacity: activeIndex === null ? 0 : 1,
      transform: `translateY(${
        activeIndex === null ? 0 : activeIndex * (itemSize + gap) + (itemSize - barSize) / 2
      }px)`,
      transition: 'transform 220ms var(--ease-spring-smooth), opacity 120ms ease',
    }}
  />
);

export interface PageShellProps {
  /** 页面名称，进入 40px 顶栏。 */
  title: string;
  /** 一句说明，放在正文顶部。 */
  description?: string;
  /** 顶栏右侧动作区。 */
  actions?: React.ReactNode;
  /**
   * 正文是否吃满余下高度。
   *
   * 默认是一块可滚动的普通文档流（列表、表单这类内容自己决定高度）。
   * 传 true 时正文区改为固定高度的纵向 flex，页面内部再用 `flex-1` 分配空间：
   * 编辑器、时间轮这类「主体本身就该占满」的页面用这一档。
   */
  fill?: boolean;
  children: React.ReactNode;
}

/**
 * 非对话页的统一外壳。
 *
 * 顶栏与对话页的指标条同高（40px）同线，支持沉浸式窗口拖拽并自动避让右上角窗口控制区。
 */
export const PageShell: React.FC<PageShellProps> = ({
  title,
  description,
  actions,
  fill = false,
  children,
}) => (
  <main className="flex-1 min-w-0 h-full flex flex-col bg-canvas overflow-clip">
    <header
      data-tauri-drag-region
      style={{ paddingRight: 'var(--window-controls-inset, 0px)' }}
      className="shrink-0 h-10 hairline-b border-border-hairline select-none"
    >
      <div className="mx-auto flex h-10 w-full max-w-page items-center gap-3 px-6">
        <Text size="sm" strong truncate>
          {title}
        </Text>
        {actions !== undefined && (
          <div className="ml-auto shrink-0 flex items-center gap-1.5">{actions}</div>
        )}
      </div>
    </header>
    <div className={`flex-1 min-h-0 ${fill ? 'overflow-clip' : 'overflow-y-auto'}`}>
      <div
        className={`mx-auto w-full max-w-page px-6 py-5 ${
          fill ? 'flex h-full min-h-0 flex-col' : ''
        }`}
      >
        {description !== undefined && (
          <Text tone="secondary" size="xs" className="mb-4 block">
            {description}
          </Text>
        )}
        {children}
      </div>
    </div>
  </main>
);
