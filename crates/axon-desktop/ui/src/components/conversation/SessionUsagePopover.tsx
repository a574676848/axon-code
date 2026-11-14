import React from 'react';
import {
  cumulativeCacheHitBasisPoints,
  type SessionCumulative,
} from '../../conversation/sessionUsage';
import { formatBasisPointsPercent, formatDecimalCount } from '../../conversation/format';
import { DatabaseIcon } from '../../icons';
import { HoverCard, SURFACE_CARD_WIDTH } from '../ui/HoverCard';
import { MetricUsageCard, type MetricUsageBarSegment, type MetricUsageRow } from './MetricUsageCard';

/*
 * 会话累计用量弹窗：指针停在指标条的「累计」段时展开。
 *
 * 与上下文浮层同一张卡：合计在右上角，中间一条比例色带，发丝线之下是分桶明细。
 * 一张卡只回答一件事：这条会话的 token 花在哪了。合计是整条会话的累计量（不是最近一轮，
 * 也不是上下文窗口的占用），色带与明细都按来源拆开；缓存命中率单独一行，它是「省了多少」
 * 的比例，不参与求和，因此不给色块。
 *
 * 颜色取主题的图表色板（两套主题各自成组），因此缓存读取、未缓存输入、输出与推理
 * 在任何主题下都有可辨且互不相同的标记。
 *
 * 读数与上下文浮层同一套单位口径：量级压缩成 K / M 并跟随数值（`2.17M`）。
 *
 * 卡片不接收指针事件，指针离开入口即收起，与行悬浮卡同款。
 */

/** 分桶与颜色的对应：图表色板里已经按 token 来源分好色，直接沿用。 */
const COLORS = {
  input: 'var(--color-chart-input)',
  cacheRead: 'var(--color-chart-cache-read)',
  cacheWrite: 'var(--color-chart-cache-write)',
  output: 'var(--color-chart-output)',
  reasoning: 'var(--color-chart-reasoning)',
} as const;

export const SessionUsagePopover: React.FC<{
  cumulative: SessionCumulative;
  children: React.ReactNode;
}> = ({ cumulative, children }) => (
  <HoverCard
    tone="surface"
    width={SURFACE_CARD_WIDTH}
    content={
      <MetricUsageCard
        icon={<DatabaseIcon size={14} />}
        title="Token 用量"
        total={formatDecimalCount(cumulative.totalTokens)}
        bar={barSegments(cumulative)}
        rows={rows(cumulative)}
      />
    }
  >
    {children}
  </HoverCard>
);

/** 分桶及其占比：为零的桶不进色带也不占行，合计始终是全部之和。 */
function buckets(cumulative: SessionCumulative): {
  key: string;
  label: string;
  tokens: number;
  color: string;
}[] {
  const all = [
    { key: 'cacheRead', label: '缓存读取', tokens: cumulative.cacheReadTokens, color: COLORS.cacheRead },
    { key: 'input', label: '未缓存输入', tokens: cumulative.inputTokens, color: COLORS.input },
    { key: 'cacheWrite', label: '缓存写入', tokens: cumulative.cacheWriteTokens, color: COLORS.cacheWrite },
    { key: 'output', label: '输出', tokens: cumulative.outputTokens, color: COLORS.output },
    { key: 'reasoning', label: '推理', tokens: cumulative.reasoningTokens, color: COLORS.reasoning },
  ];
  return all.filter((bucket) => bucket.tokens > 0);
}

/** 色带：按各桶占全部用量的比例铺开，零头补给最大的一段，铺满整条。 */
function barSegments(cumulative: SessionCumulative): MetricUsageBarSegment[] {
  const list = buckets(cumulative);
  const total = list.reduce((sum, bucket) => sum + bucket.tokens, 0);
  if (total <= 0) return [];
  let assigned = 0;
  const segments = list.map((bucket) => {
    const percent = Math.min(100 - assigned, Math.floor((bucket.tokens * 100) / total));
    assigned += percent;
    return { key: bucket.key, percent, color: bucket.color };
  });
  const leftover = 100 - assigned;
  if (leftover > 0 && segments.length > 0) segments[0].percent += leftover;
  return segments;
}

/** 明细行：命中率在前（比例，无色块），其后是各桶。 */
function rows(cumulative: SessionCumulative): MetricUsageRow[] {
  return [
    {
      label: '缓存命中',
      value: formatBasisPointsPercent(cumulativeCacheHitBasisPoints(cumulative)),
      hint: '缓存读取占完整输入的会话累计比例',
    },
    ...buckets(cumulative).map((bucket) => ({
      label: bucket.label,
      value: formatDecimalCount(bucket.tokens),
      color: bucket.color,
    })),
  ];
}
