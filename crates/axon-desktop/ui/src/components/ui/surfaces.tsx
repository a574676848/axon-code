import React from 'react';

/*
 * 外观组件层：把重复出现两次以上的视觉组合收敛成组件。
 * 业务组件只声明语义（这是提示条、这是列表行），不再各自拼装描边、留白与字色，
 * 因此全站样式只有一个来源，改一处即全站生效。
 */

// ------------------------------------------------------------------ 文本

type TextTone = 'primary' | 'secondary' | 'tertiary' | 'caption' | 'danger';
type TextSize = 'xs' | 'sm' | 'base' | '2xs' | 'code' | 'meta' | 'row' | 'body' | 'content';

const TEXT_TONE: Record<TextTone, string> = {
  primary: 'text-text-primary',
  secondary: 'text-text-secondary',
  tertiary: 'text-text-tertiary',
  caption: 'text-text-caption',
  danger: 'text-danger',
};

const TEXT_SIZE: Record<TextSize, string> = {
  '2xs': 'text-2xs',
  code: 'text-code',
  xs: 'text-xs',
  meta: 'text-meta',
  row: 'text-row',
  sm: 'text-sm',
  body: 'text-body',
  content: 'text-content',
  base: 'text-base',
};

export interface TextProps {
  /** 语义色。 */
  tone?: TextTone;
  /** 字号档位。 */
  size?: TextSize;
  /** 是否使用等宽字体。 */
  mono?: boolean;
  /** 是否单行截断。 */
  truncate?: boolean;
  /** 是否使用中等字重。 */
  strong?: boolean;
  /** 附加类名，仅用于布局（间距、宽度），不用于外观。 */
  className?: string;
  /** 原生 title 提示。 */
  title?: string;
  children: React.ReactNode;
}

/**
 * 文本基元。
 *
 * 转发 ref 以便外层直接操作元素本身（例如标题跑马灯需要读写滚动位置）。
 */
export const Text = React.forwardRef<HTMLSpanElement, TextProps>(function Text(
  {
    tone = 'primary',
    size = 'sm',
    mono = false,
    truncate = false,
    strong = false,
    className = '',
    title,
    children,
  },
  ref,
) {
  return (
    <span
      ref={ref}
      title={title}
      className={`${TEXT_SIZE[size]} ${TEXT_TONE[tone]} ${mono ? 'font-mono' : ''} ${
        strong ? 'font-medium' : ''
      } ${truncate ? 'block truncate' : ''} ${className}`}
    >
      {children}
    </span>
  );
});

// ------------------------------------------------------------------ 容器

/**
 * 可交互条目的唯一悬浮规则。
 * 列表行、菜单项、下拉选项、导航项都引用它，保证全站悬浮反馈完全一致。
 */
export const INTERACTIVE_ROW = 'transition-colors duration-100 hover:bg-hover';

export interface PanelProps {
  /** 附加类名，仅用于布局。 */
  className?: string;
  /** 是否裁剪内部溢出的内容（列表场景使用）。 */
  clip?: boolean;
  children: React.ReactNode;
}

/** 描边容器：白底、发丝描边、光学高光棱与模块圆角，是全站唯一的区块外观。 */
export const Panel: React.FC<PanelProps> = ({ className = '', clip = false, children }) => (
  <div
    className={`bg-surface hairline border-border shadow-card rounded-md ${
      clip ? 'overflow-hidden' : ''
    } ${className}`}
  >
    {children}
  </div>
);

/** 发丝分隔线。 */
export const Hairline: React.FC<{ className?: string }> = ({ className = '' }) => (
  <div className={`h-hairline bg-border-hairline ${className}`} role="separator" />
);

/** 纵向堆叠的发丝分隔（用于配置项列表）。 */
export const HairlineStack: React.FC<{ className?: string; children: React.ReactNode }> = ({
  className = '',
  children,
}) => (
  <div
    className={`divide-hairline divide-border-hairline hairline-y border-border-hairline ${className}`}
  >
    {children}
  </div>
);

export interface ListRowProps {
  /** 是否处于选中态。 */
  selected?: boolean;
  /** 点击行为。 */
  onClick?: () => void;
  /** 附加类名，仅用于布局。 */
  className?: string;
  children: React.ReactNode;
}

/** 列表行：悬浮与选中都是同一档浅灰底，是列表交互的唯一视觉规则。 */
export const ListRow: React.FC<ListRowProps> = ({
  selected = false,
  onClick,
  className = '',
  children,
}) => (
  <div
    onClick={onClick}
    className={`group flex items-center gap-3 px-3 py-2.5 hairline-b border-border-hairline last:border-b-0 ${INTERACTIVE_ROW} ${
      selected ? 'bg-active' : ''
    } ${onClick !== undefined ? 'cursor-pointer' : ''} ${className}`}
  >
    {children}
  </div>
);

/**
 * 行内悬浮才出现的操作区。
 *
 * 用显示切换而不是可见性切换：`invisible` 仍然占据布局宽度，
 * 会让操作图标常驻吃掉标题的空间，静止时标题提前被截断。
 * 显示切换在悬浮瞬间让出宽度给标题，与既有工作台产品一致；
 * 菜单展开期间由 `active` 强制常显，因此不会连带销毁已打开的菜单。
 */
export const RowActions: React.FC<{
  /** 操作区内部有展开的菜单时，强制常显。 */
  active?: boolean;
  className?: string;
  children: React.ReactNode;
}> = ({ active = false, className = '', children }) => (
  <div
    className={`shrink-0 items-center gap-0.5 ${
      active ? 'flex' : 'hidden group-hover:flex group-focus-within:flex'
    } ${className}`}
  >
    {children}
  </div>
);

// ------------------------------------------------------------------ 状态标识

/**
 * 状态语义：完成是绿点，需要人过目是琥珀点，出错是红点，
 * 既没在跑也没结果是灰点；在跑的是转圈，因为圆点不表达「正在进行」。
 */
export type StateDotState = 'done' | 'warning' | 'error' | 'idle' | 'ongoing';

const DOT_STATE: Record<Exclude<StateDotState, 'ongoing'>, string> = {
  done: 'text-ok',
  warning: 'text-gold',
  error: 'text-danger',
  idle: 'text-state-idle',
};

/** 标记槽边长：实心状态 10px，转圈 14px，与参考实现的取值一致。 */
const DOT_EDGE = 10;
const ONGOING_EDGE = 14;

/**
 * 转圈：把动画起点钉到时间零点。
 *
 * CSS 动画从元素插入时起算，不同时刻挂上来的转圈会各转各的相位；
 * 共用一个起点，同屏可见的转圈就同步转。
 */
function alignSpinnerPhase(element: SVGSVGElement | null): void {
  if (element === null) return;
  // 无头测试环境（jsdom）没有 Web Animations，取不到就保持未对齐，不影响渲染。
  const spinner = element as { getAnimations?: SVGSVGElement['getAnimations'] };
  for (const animation of spinner.getAnimations?.({ subtree: true }) ?? []) {
    animation.startTime = 0;
  }
}

/**
 * 状态标识：会话、子代理与后台作业共用的一份状态点。
 *
 * 实心状态在 10px 的槽里画一颗 6px 的圆点，槽宽恒定，
 * 因此换成转圈或别的状态时行内文字不会横跳。
 */
export const StateDot: React.FC<{ state: StateDotState; className?: string }> = ({
  state,
  className = '',
}) => {
  if (state === 'ongoing') {
    return (
      <svg
        ref={alignSpinnerPhase}
        className={`state-spinner shrink-0 text-text-tertiary ${className}`}
        width={ONGOING_EDGE}
        height={ONGOING_EDGE}
        viewBox="0 0 24 24"
        aria-hidden="true"
      >
        <g className="state-spinner-motion">
          <circle className="state-spinner-track" cx="12" cy="12" r="9.5" />
          <circle className="state-spinner-arc" cx="12" cy="12" r="9.5" />
        </g>
      </svg>
    );
  }
  return (
    <span
      className={`shrink-0 inline-flex items-center justify-center ${DOT_STATE[state]} ${className}`}
      style={{ width: DOT_EDGE, height: DOT_EDGE }}
      aria-hidden="true"
    >
      <span className="w-1.5 h-1.5 rounded-full bg-current" />
    </span>
  );
};

// ------------------------------------------------------------------ 提示

type BannerTone = 'success' | 'danger';

const BANNER_ICON_COLOR: Record<BannerTone, TextTone> = {
  success: 'secondary',
  danger: 'danger',
};

export interface BannerProps {
  /** 提示语气。 */
  tone: BannerTone;
  /** 前置图标。 */
  icon: React.ReactNode;
  /** 主文案。 */
  title: string;
  /** 明细列表，例如逐条校验错误。 */
  details?: string[];
  /** 右侧或底部的可选操作。 */
  action?: React.ReactNode;
}

/** 状态提示条：统一承载校验结论、保存结果与加载错误。 */
export const Banner: React.FC<BannerProps> = ({ tone, icon, title, details = [], action }) => (
  <Panel className="p-2.5 flex items-start gap-2">
    <span className={`shrink-0 mt-0.5 ${TEXT_TONE[BANNER_ICON_COLOR[tone]]}`}>{icon}</span>
    <div className="flex-1 min-w-0 space-y-1">
      <Text tone={tone === 'danger' ? 'danger' : 'primary'} size="xs">
        {title}
      </Text>
      {details.map((detail, index) => (
        <Text key={index} tone="danger" size="xs" mono className="block break-all">
          {detail}
        </Text>
      ))}
      {action}
    </div>
  </Panel>
);

/** 居中的弱提示，用于空列表与加载中。 */
export const EmptyHint: React.FC<{ className?: string; children: React.ReactNode }> = ({
  className = '',
  children,
}) => (
  <div className={`py-6 text-center ${className}`}>
    <Text tone="tertiary" size="xs">
      {children}
    </Text>
  </div>
);

/**
 * 空态：图标、主句、可选次句与动作，全站同一几何。
 *
 * 空态是页面的一种正式状态，不是一句灰字：统一放在这里，
 * 各页只声明「缺什么、为什么缺、下一步能做什么」。
 */
export interface EmptyStateProps {
  /** 顶部图形；不给则只排文字。 */
  icon?: React.ReactNode;
  /** 主句：说明缺什么。 */
  title: string;
  /** 次句：说明为什么缺，或下一步。 */
  description?: string;
  /** 可选动作。 */
  action?: React.ReactNode;
  className?: string;
}

export const EmptyState: React.FC<EmptyStateProps> = ({
  icon,
  title,
  description,
  action,
  className = '',
}) => (
  <div className={`flex flex-col items-center justify-center gap-2 px-6 py-10 text-center ${className}`}>
    {icon !== undefined && <span className="text-text-tertiary">{icon}</span>}
    <Text size="body" strong>
      {title}
    </Text>
    {description !== undefined && (
      <Text tone="secondary" size="xs" className="max-w-md">
        {description}
      </Text>
    )}
    {action !== undefined && <div className="mt-1.5">{action}</div>}
  </div>
);

/** 加载态：与空态同高同轴，避免状态切换时版面跳动。 */
export const LoadingState: React.FC<{ label: string; className?: string }> = ({
  label,
  className = '',
}) => (
  <div className={`flex items-center justify-center gap-2 px-6 py-10 ${className}`}>
    <span className="state-spinner text-text-tertiary" aria-hidden="true">
      <svg width="14" height="14" viewBox="0 0 24 24">
        <g className="state-spinner-motion">
          <circle className="state-spinner-track" cx="12" cy="12" r="9.5" />
          <circle className="state-spinner-arc" cx="12" cy="12" r="9.5" />
        </g>
      </svg>
    </span>
    <Text tone="secondary" size="xs">
      {label}
    </Text>
  </div>
);

/** 小型状态标签（协议、只读、视觉支持等）。 */
export const Chip: React.FC<{ className?: string; children: React.ReactNode }> = ({
  className = '',
  children,
}) => (
  <span
    className={`shrink-0 inline-flex items-center px-1.5 py-px rounded-xs bg-surface-sunken/60 hairline border-border-hairline ${className}`}
  >
    <Text tone="tertiary" size="xs">
      {children}
    </Text>
  </span>
);

// ------------------------------------------------------------------ 表单

/** 表单字段：标签在上，控件在下，是表单里唯一的字段外观。 */
export const FormField: React.FC<{ label: string; className?: string; children: React.ReactNode }> = ({
  label,
  className = '',
  children,
}) => (
  <label className={`block space-y-1.5 ${className}`}>
    <Text tone="secondary" size="xs" className="block">
      {label}
    </Text>
    {children}
  </label>
);

/** 开关行：左说明右控件；说明可选，与配置页同一套两级排版。 */
export const ToggleField: React.FC<{
  label: string;
  description?: string;
  control: React.ReactNode;
}> = ({ label, description, control }) => (
  <div className="flex items-center justify-between gap-3">
    <div className="min-w-0 space-y-0.5">
      <Text size="sm">{label}</Text>
      {description !== undefined && (
        <Text tone="secondary" size="xs" className="block">
          {description}
        </Text>
      )}
    </div>
    {control}
  </div>
);

/** 区块标题：标题加一句说明。 */
export const SectionHeading: React.FC<{
  title: string;
  description?: string;
  meta?: React.ReactNode;
  action?: React.ReactNode;
  className?: string;
}> = ({ title, description, meta, action, className = '' }) => (
  <div className={`flex items-start justify-between gap-3 ${className}`}>
    <div className="min-w-0 space-y-0.5">
      <Text size="sm" strong>
        {title}
      </Text>
      {description !== undefined && (
        <Text tone="secondary" size="xs" className="block">
          {description}
        </Text>
      )}
      {meta}
    </div>
    {action}
  </div>
);
