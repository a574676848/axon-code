import React from 'react';
import { type ContextUsageModel } from '../../conversation/contextUsage';
import { RingIcon } from '../../icons';
import { HoverCard, SURFACE_CARD_WIDTH } from '../ui/HoverCard';
import { MetricUsageCard, type MetricUsageRow } from './MetricUsageCard';

/*
 * 上下文使用弹窗：指针停住指标条上的「上下文」段时展开。
 *
 * 与用量弹窗同一张卡、同一套排版：标题 + 已用/窗口，发丝线之下是构成明细。
 * 这里不画色带也不铺色块：色带要靠「段落宽度」表达占比，而构成是本地估算、总量是
 * Provider 实测，两者口径天然不同，画成一条实心带只会让人以为它们严丝合缝。
 * 估算值一律带 `~`，实测总量不带，两者不混为一谈。
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
        title="上下文已用"
        total={model.totalText}
        rows={model.rows.map(
          (row): MetricUsageRow => ({ label: row.label, value: row.value }),
        )}
        fallback="本轮未观测到上下文构成"
      />
    }
  >
    {children}
  </HoverCard>
);
