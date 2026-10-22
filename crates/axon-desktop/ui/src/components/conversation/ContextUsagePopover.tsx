import React from 'react';
import { segmentValueText, type ContextUsageModel } from '../../conversation/contextUsage';
import { HoverCard, SURFACE_CARD_WIDTH } from '../ui/HoverCard';

/*
 * 上下文使用弹窗：指针停住指标条上的「上下文」段时展开。
 *
 * 三件事按重要性自上而下排：已用百分比（一眼）、实测总量与窗口（一眼）、
 * 构成分段与色带（想知道「都被谁占掉了」时才看）。估算值一律带 `~`，
 * Provider 实测总量不带，两者不混为一谈。
 *
 * 卡片不接收指针事件，指针离开入口即收起，与行悬浮卡同款。
 */

export const ContextUsagePopover: React.FC<{
  model: ContextUsageModel;
  children: React.ReactNode;
}> = ({ model, children }) => (
  <HoverCard
    tone="surface"
    width={SURFACE_CARD_WIDTH}
    content={<ContextUsageCard model={model} />}
  >
    {children}
  </HoverCard>
);

const ContextUsageCard: React.FC<{ model: ContextUsageModel }> = ({ model }) => (
  <>
    <div className="flex items-baseline justify-between gap-3">
      <span className="flex items-baseline gap-1.5 text-row text-text-secondary">
        上下文已用
        <span className="text-body font-medium text-text-primary tabular-nums">
          {model.usedPercent === null ? '未知' : `${model.usedPercent}%`}
        </span>
      </span>
      <span className="shrink-0 text-meta text-text-secondary tabular-nums">
        {model.windowText}
      </span>
    </div>
    {model.hasComposition ? (
      <>
        <div className="h-1.5 rounded-full bg-surface-sunken overflow-hidden flex">
          {model.segments.map((segment) => (
            <span
              key={segment.kind}
              title={segment.hint}
              style={{ width: `${segment.barPercent}%` }}
              className={`h-full ${segment.swatchClassName}`}
            />
          ))}
        </div>
        <div className="flex flex-col gap-1.5">
          {model.segments.map((segment) => (
            <div key={segment.kind} className="flex items-center gap-2">
              <span
                aria-hidden="true"
                className={`shrink-0 w-2 h-2 rounded-xs ${segment.swatchClassName}`}
              />
              <span className="flex-1 min-w-0 truncate text-row text-text-secondary" title={segment.hint}>
                {segment.label}
              </span>
              <span className="shrink-0 text-row text-text-primary tabular-nums">
                {segmentValueText(segment)}
              </span>
            </div>
          ))}
        </div>
      </>
    ) : (
      // 降级必须可见：这一轮没有构成观测就直说，不用 0 或占位条冒充。
      <span className="text-meta text-text-tertiary">本轮未观测到上下文构成</span>
    )}
    {/* 缓存明细从指标条收进来：同一块预算的第二层读数，只有展开时才需要。 */}
    <div className="flex flex-col gap-1 pt-1 border-t border-border-hairline">
      {model.cacheRows.map((row) => (
        <div key={row.label} className="flex items-center gap-2">
          <span className="flex-1 min-w-0 truncate text-meta text-text-tertiary">{row.label}</span>
          <span className="shrink-0 text-meta text-text-secondary tabular-nums">{row.value}</span>
        </div>
      ))}
    </div>
  </>
);
