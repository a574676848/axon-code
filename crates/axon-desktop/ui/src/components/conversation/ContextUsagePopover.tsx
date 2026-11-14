import React from 'react';
import { segmentValueText, type ContextUsageModel } from '../../conversation/contextUsage';
import { RingIcon } from '../../icons';
import { HoverCard, SURFACE_CARD_WIDTH } from '../ui/HoverCard';
import { MetricUsageCard, type MetricUsageBarSegment, type MetricUsageRow } from './MetricUsageCard';

/*
 * 上下文使用弹窗：指针停住指标条上的「上下文」段时展开。
 *
 * 与用量弹窗同一张卡：标题带上已用百分比，右上角是已用/窗口，中间一条比例色带，
 * 发丝线之下是构成明细，每行带同色块。
 *
 * 色带表达的是「这次请求的窗口被谁占掉了」，因此标尺是上下文窗口；构成是本地估算、
 * 总量是 Provider 实测，两者口径天然不同，估算值一律带 `~`，差额单独成行。
 *
 * 缓存明细不在这里出现：它是会话累计用量的一部分，收在「累计」那一段的浮层里。
 */

export const ContextUsagePopover: React.FC<{
  model: ContextUsageModel;
  children: React.ReactNode;
}> = ({ model, children }) => (
  <HoverCard
    tone="surface"
    width={SURFACE_CARD_WIDTH}
    content={
      <MetricUsageCard
        icon={<RingIcon size={14} ratio={model.usedRatio} />}
        title={
          <>
            上下文已用
            {model.percentText !== null && (
              <span className="ml-1.5 font-medium text-text-primary tabular-nums">
                {model.percentText}
              </span>
            )}
          </>
        }
        total={model.totalText}
        bar={barSegments(model)}
        rows={rows(model)}
        fallback="本轮未观测到上下文构成"
      />
    }
  >
    {children}
  </HoverCard>
);

/** 色带：只画有观测的分段。 */
function barSegments(model: ContextUsageModel): MetricUsageBarSegment[] {
  return model.segments.map((segment) => ({
    key: segment.kind,
    percent: segment.barPercent,
    color: segment.color,
  }));
}

/** 明细行：每行带与色带同色的色块。 */
function rows(model: ContextUsageModel): MetricUsageRow[] {
  return model.segments.map((segment) => ({
    label: segment.label,
    value: segmentValueText(segment),
    color: segment.color,
    hint: segment.hint,
  }));
}
