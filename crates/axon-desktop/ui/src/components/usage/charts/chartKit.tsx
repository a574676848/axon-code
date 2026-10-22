import React from 'react';
import { Panel, Text } from '../../ui/surfaces';
import { PanelHeader } from '../../settings/PanelHeader';

/*
 * 图表外壳：标题行、绘图区、图例与脚注的统一排布。
 *
 * 全站的用量图形共用这几件东西，因此「图例怎么排、空态怎么写、脚注放哪」
 * 只有一处实现。图一律画在 `--color-chart-plot` 的绘图区底板上：白色网格线
 * 只有在比画布深半档的底上才是可读的分度。
 *
 * 绘图区里的几何一律用百分比坐标（不设 viewBox），因此：
 * 1. 容器多宽图就多宽，不需要测量 DOM，也不需要 ResizeObserver；
 * 2. 描边与圆点不被非等比缩放拉扁，圆就是圆；
 * 3. 面积与折线用 polygon 与 polyline（path 的 d 不接受百分比，这两者接受）。
 */

/** 图例项：色块加名称，可选带读数。 */
export interface LegendItem {
  label: string;
  color: string;
  value?: string;
}

/** 图例：逐项带名称，因此信息不依赖颜色本身。 */
export const ChartLegend: React.FC<{ items: LegendItem[]; className?: string }> = ({
  items,
  className = '',
}) => (
  <div className={`flex flex-wrap items-center gap-x-3 gap-y-1.5 ${className}`}>
    {items.map((item) => (
      <span key={item.label} className="inline-flex items-center gap-1.5">
        <span
          className="w-2 h-2 rounded-[2px] shrink-0"
          style={{ backgroundColor: item.color }}
          aria-hidden="true"
        />
        <Text tone="secondary" size="meta">
          {item.label}
        </Text>
        {item.value !== undefined && (
          <Text tone="tertiary" size="meta" mono>
            {item.value}
          </Text>
        )}
      </span>
    ))}
  </div>
);

/** 绘图区空态：说明缺什么，不留空壳。 */
export const EmptyPlot: React.FC<{ height?: number; children: React.ReactNode }> = ({
  height = 120,
  children,
}) => (
  <div
    className="flex items-center justify-center rounded-sm bg-chart-plot hairline border-border-hairline"
    style={{ height }}
  >
    <Text tone="tertiary" size="row">
      {children}
    </Text>
  </div>
);

export interface ChartCardProps {
  title: string;
  /** 标题下的一行说明：这张图读的是什么。 */
  description?: string;
  /** 标题行右侧的操作区（周期、维度、指标切换）。 */
  action?: React.ReactNode;
  /** 图例；给了就画在绘图区之下。 */
  legend?: LegendItem[];
  /** 图例右侧的合计读数。 */
  meta?: React.ReactNode;
  /** 图为空时的说明文案；给出后不渲染 children。 */
  empty?: string;
  /** 空态高度。 */
  emptyHeight?: number;
  /** 脚注：口径说明、截断与抽稀披露。 */
  footnote?: React.ReactNode;
  className?: string;
  children?: React.ReactNode;
}

/** 面板外壳：用量看板的每一张图都是这一个形状。 */
export const ChartCard: React.FC<ChartCardProps> = ({
  title,
  description,
  action,
  legend,
  meta,
  empty,
  emptyHeight,
  footnote,
  className = '',
  children,
}) => {
  const hasFooter = legend !== undefined || meta !== undefined || footnote !== undefined;
  return (
    /*
     * 一块面板分三层，层与层之间用发丝线切开：标题层（这张图是什么）、
     * 图形层（画的是什么）、读数层（图例、合计与口径）。看板整体铺在
     * surface-sunken 底板上，面板取白底加柔光，因此每一块都是一个有边界的
     * 独立单元，而不是靠留白勉强分开的一片白。
     */
    <Panel className={`flex flex-col min-w-0 shadow-panel ${className}`}>
      <PanelHeader title={title} description={description} action={action} />

      <div className="flex-1 min-w-0 px-3.5 py-3">
        {empty !== undefined ? (
          <EmptyPlot height={emptyHeight}>{empty}</EmptyPlot>
        ) : (
          <div className="min-w-0">{children}</div>
        )}
      </div>

      {hasFooter && (
        <div className="shrink-0 px-3.5 py-2.5 space-y-1.5 bg-surface-sunken hairline-t border-border-hairline">
          {(legend !== undefined || meta !== undefined) && (
            <div className="flex items-start justify-between gap-3">
              {legend !== undefined ? <ChartLegend items={legend} /> : <span />}
              {meta !== undefined && <div className="shrink-0">{meta}</div>}
            </div>
          )}
          {footnote !== undefined && (
            <Text tone="tertiary" size="meta" className="block">
              {footnote}
            </Text>
          )}
        </div>
      )}
    </Panel>
  );
};

/** 绘图区：相对定位的底板，内部图形一律绝对铺满。 */
export const PlotArea: React.FC<{
  height: number;
  /** 左侧刻度文案，自上而下对应各条网格线。 */
  yLabels?: string[];
  /** 底部刻度：`ratio` 为 0 至 1 的横向位置。 */
  xLabels?: { label: string; ratio: number }[];
  children: React.ReactNode;
}> = ({ height, yLabels, xLabels, children }) => (
  <div className="flex gap-2 min-w-0">
    {yLabels !== undefined && (
      <div
        className="shrink-0 w-14 flex flex-col justify-between items-end text-right"
        style={{ height }}
        aria-hidden="true"
      >
        {yLabels.map((label, index) => (
          <Text key={`${label}-${index}`} tone="tertiary" size="2xs" mono>
            {label}
          </Text>
        ))}
      </div>
    )}
    <div className="flex-1 min-w-0">
      <div
        className="relative rounded-sm bg-chart-plot hairline border-border-hairline overflow-hidden"
        style={{ height }}
      >
        {children}
      </div>
      {xLabels !== undefined && (
        <div className="relative h-4 mt-1" aria-hidden="true">
          {xLabels.map((tick) => (
            <span
              key={`${tick.label}-${tick.ratio}`}
              className="absolute top-0 -translate-x-1/2 whitespace-nowrap"
              style={{ left: `${clamp(tick.ratio, 0.02, 0.98) * 100}%` }}
            >
              <Text tone="tertiary" size="2xs" mono>
                {tick.label}
              </Text>
            </span>
          ))}
        </div>
      )}
    </div>
  </div>
);

/** 网格线：等分的水平细线，画在图形之下。 */
export const GridLines: React.FC<{ count: number }> = ({ count }) => (
  <svg className="absolute inset-0 w-full h-full" aria-hidden="true">
    {Array.from({ length: count + 1 }, (_, index) => {
      const ratio = index / count;
      return (
        <line
          key={index}
          x1="0"
          x2="100%"
          y1={`${ratio * 100}%`}
          y2={`${ratio * 100}%`}
          style={{ stroke: 'var(--color-chart-grid)' }}
          strokeWidth={1}
        />
      );
    })}
  </svg>
);

/** 把值夹到区间内。 */
export function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, value));
}

/**
 * 悬浮读数的定位。
 *
 * 贴边的读数不再居中：居中的气泡在两端会被裁掉一半，档位切换比裁切好读。
 */
export function tooltipPlacement(ratio: number): { left: string; transform: string } {
  if (ratio < 0.12) return { left: '0%', transform: 'translateX(0)' };
  if (ratio > 0.88) return { left: '100%', transform: 'translateX(-100%)' };
  return { left: `${ratio * 100}%`, transform: 'translateX(-50%)' };
}

/** 悬浮读数气泡。 */
export const ChartTooltip: React.FC<{ ratio: number; children: React.ReactNode }> = ({
  ratio,
  children,
}) => (
  <div
    className="absolute top-2 z-10 pointer-events-none rounded-sm bg-surface shadow-panel hairline border-border-hairline px-2 py-1.5 space-y-1 min-w-[132px]"
    style={tooltipPlacement(ratio)}
    role="status"
  >
    {children}
  </div>
);

/** 悬浮读数里的一行：左标签右读数。 */
export const TooltipRow: React.FC<{ label: string; value: string; color?: string }> = ({
  label,
  value,
  color,
}) => (
  <div className="flex items-center justify-between gap-3">
    <span className="inline-flex items-center gap-1.5 min-w-0">
      {color !== undefined && (
        <span
          className="w-2 h-2 rounded-[2px] shrink-0"
          style={{ backgroundColor: color }}
          aria-hidden="true"
        />
      )}
      <Text tone="secondary" size="2xs" className="truncate">
        {label}
      </Text>
    </span>
    <Text size="2xs">
      {value}
    </Text>
  </div>
);
