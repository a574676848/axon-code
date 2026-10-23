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

/** 侧边栏列：展开与收起共享同一底色、描边与层级。 */
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
    className={`relative flex flex-col shrink-0 h-full bg-sidebar hairline-r border-border-hairline z-10 ${
      variant === 'rail' ? 'w-14 items-center py-3' : ''
    } ${className}`}
  >
    {children}
  </aside>
);

export interface NewSessionButtonProps {
  /** 点击行为。 */
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
        className={`w-9 h-9 flex items-center justify-center rounded-md hairline border-border-strong bg-surface text-text-primary ${INTERACTIVE_ROW}`}
      >
        <IconNewChat size={16} />
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={onClick}
      className={`group relative w-full h-9 px-3 flex items-center justify-start rounded-md hairline border-border-strong bg-surface ${INTERACTIVE_ROW}`}
    >
      {/* 图标与文案整体居中，快捷键提示绝对定位，出现时不会把文案挤偏 */}
      <span className="flex items-center gap-1.5 min-w-0">
        <IconNewChat size={15} className="shrink-0" />
        <Text strong truncate>
          新会话
        </Text>
      </span>
      <Text
        tone="tertiary"
        size="xs"
        className="absolute right-3 top-1/2 -translate-y-1/2 opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-focus-visible:opacity-100"
      >
        Ctrl + Alt + N
      </Text>
    </button>
  );
};

export interface PageShellProps {
  /** 页面名称，进入 36px 顶栏。 */
  title: string;
  /** 一句说明，放在正文顶部。 */
  description?: string;
  /** 顶栏右侧动作区。 */
  actions?: React.ReactNode;
  children: React.ReactNode;
}

/**
 * 非对话页的统一外壳。
 *
 * 顶栏与对话页的指标条同高（36px）同线，正文走同一条居中版心：
 * 切页时顶部结构与左右轴线都不再改变，页面之间只剩内容差异。
 */
export const PageShell: React.FC<PageShellProps> = ({
  title,
  description,
  actions,
  children,
}) => (
  <main className="flex-1 min-w-0 h-full flex flex-col bg-canvas overflow-clip">
    <header className="shrink-0 h-9 hairline-b border-border-hairline">
      {/* 顶栏与正文共用同一条版心：宽屏下标题与内容左轴必须重合，
          否则页头贴窗口左边、正文居中，中间会空出一道无主的留白。 */}
      <div className="mx-auto flex h-9 w-full max-w-page items-center gap-3 px-6">
        <Text size="sm" strong truncate>
          {title}
        </Text>
        {actions !== undefined && (
          <div className="ml-auto shrink-0 flex items-center gap-1.5">{actions}</div>
        )}
      </div>
    </header>
    <div className="flex-1 min-h-0 overflow-y-auto">
      <div className="mx-auto w-full max-w-page px-6 py-5">
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
