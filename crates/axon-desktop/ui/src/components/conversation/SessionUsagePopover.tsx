import React from 'react';
import {
  cumulativeCacheHitBasisPoints,
  type SessionCumulative,
} from '../../conversation/sessionUsage';
import { formatBasisPointsPercent, formatDecimalCount } from '../../conversation/format';
import { DatabaseIcon } from '../../icons';
import { HoverCard, SURFACE_CARD_WIDTH } from '../ui/HoverCard';
import { MetricUsageCard, type MetricUsageRow } from './MetricUsageCard';

/*
 * 会话累计用量弹窗：指针停在指标条的「累计」段时展开。
 *
 * 一张卡只回答一件事：这条会话的 token 花在哪了。合计是整条会话的累计量（不是最近一轮，
 * 也不是上下文窗口的占用），下面按来源拆成未缓存输入、缓存读取、缓存写入、输出与推理；
 * 缓存命中率单独一行，它是「省了多少」的比例，不参与求和。
 *
 * 读数与上下文浮层同一套单位口径：量级压缩成 K / M 并跟随数值（`2.17M`），不在这里
 * 铺一串十一位数。展开是为了看清构成比例，不是做账。
 *
 * 卡片不接收指针事件，指针离开入口即收起，与行悬浮卡同款。
 */

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
        rows={usageRows(cumulative)}
      />
    }
  >
    {children}
  </HoverCard>
);

/** 明细行：命中率在前（它是比例），其余按来源列出，为零的桶不占行。 */
function usageRows(cumulative: SessionCumulative): MetricUsageRow[] {
  const rows: MetricUsageRow[] = [
    {
      label: '缓存命中',
      value: formatBasisPointsPercent(cumulativeCacheHitBasisPoints(cumulative)),
    },
    { label: '未缓存输入', value: formatDecimalCount(cumulative.inputTokens) },
    { label: '缓存读取', value: formatDecimalCount(cumulative.cacheReadTokens) },
  ];
  if (cumulative.cacheWriteTokens > 0) {
    rows.push({ label: '缓存写入', value: formatDecimalCount(cumulative.cacheWriteTokens) });
  }
  rows.push({ label: '输出', value: formatDecimalCount(cumulative.outputTokens) });
  if (cumulative.reasoningTokens > 0) {
    rows.push({ label: '推理', value: formatDecimalCount(cumulative.reasoningTokens) });
  }
  return rows;
}
