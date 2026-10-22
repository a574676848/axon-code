import React from 'react';
import { Text } from '../../ui/surfaces';
import { ChartCard } from './chartKit';
import type { UsageDistributionItem } from '../../../types';
import { costLabel, formatCount, formatPercent, formatRate, formatTokens } from '../usageFormat';

/*
 * 用量排行：按 Token 占比排序的横向条形。
 *
 * 条长表示占比，右侧固定四列读数（Token、占比、请求、缓存命中）。行的顺序即排名，
 * 因此不额外画名次数字：位置本身就是名次，再加一个 01 / 02 / 03 只是重复。
 *
 * 零 Token 的条目照样列出来（它们有请求、只是没产生 Token，通常是失败请求）：
 * 从榜上抹掉会让「请求数合计」对不上大盘。
 */

/** 最多列出多少条：再多就压过同屏的其他面板。 */
const MAX_ROWS = 10;

export const RankingBars: React.FC<{
  items: UsageDistributionItem[];
  dimensionLabel: string;
  /** 周期名称，用于口径说明。 */
  periodLabel: string;
  /** 维度切换控件：状态归容器，榜只负责画。 */
  action?: React.ReactNode;
}> = ({ items, dimensionLabel, periodLabel, action }) => {
  const maxTokens = Math.max(...items.map((item) => item.tokens), 0);
  const shown = items.slice(0, MAX_ROWS);
  const rest = items.slice(MAX_ROWS);
  const restTokens = rest.reduce((sum, item) => sum + item.tokens, 0);
  const restRequests = rest.reduce((sum, item) => sum + item.requestCount, 0);

  return (
    <ChartCard
      title={`${dimensionLabel}用量排行`}
      description={`${periodLabel}内按 Token 总量排序，条长表示占比`}
      action={action}
      empty={items.length === 0 ? '这一周期还没有用量记录' : undefined}
      emptyHeight={180}
      footnote={
        rest.length === 0
          ? '缓存命中率按「命中读取 ÷ 输入总量」计算，未配置价格时成本显示为未定价'
          : `仅列出前 ${MAX_ROWS} 项，其余 ${rest.length} 项合计 ${formatTokens(restTokens)} Token、${formatCount(restRequests)} 次请求`
      }
    >
      <ul className="space-y-2">
        {shown.map((item) => (
          <li key={item.key} className="flex items-center gap-3 min-w-0">
            <div className="w-[132px] shrink-0 min-w-0">
              <Text size="row" truncate title={item.key}>
                {item.key}
              </Text>
              <Text tone="tertiary" size="2xs" className="block">
                {formatCount(item.requestCount)} 次 · {formatRate(item.rpm)} RPM
              </Text>
            </div>
            <div
              className="flex-1 min-w-0 h-4 rounded-[3px] overflow-hidden"
              style={{ backgroundColor: 'var(--color-chart-track)' }}
            >
              <div
                className="bar-grow h-full rounded-[3px]"
                style={{
                  width: `${maxTokens <= 0 ? 0 : Math.max(1.5, (item.tokens / maxTokens) * 100)}%`,
                  backgroundColor: 'var(--color-chart-input)',
                }}
              />
            </div>
            <div className="shrink-0 flex items-center gap-3">
              <span className="w-[68px] text-right">
                <Text size="row" mono>
                  {formatTokens(item.tokens)}
                </Text>
              </span>
              <span className="w-[54px] text-right">
                <Text tone="secondary" size="row" mono>
                  {`${item.percentage.toFixed(2)}%`}
                </Text>
              </span>
              <span className="w-[62px] text-right">
                <Text tone="tertiary" size="row" mono>
                  {formatPercent(item.cacheHitRateBasisPoints, 0)}
                </Text>
              </span>
              <span className="w-[76px] text-right">
                <Text tone="tertiary" size="row">
                  {costLabel(item.costMicros, item.requestCount)}
                </Text>
              </span>
            </div>
          </li>
        ))}
      </ul>
      {shown.length > 0 && (
        <div className="flex items-center gap-3 mt-2 pt-1.5 hairline-t border-border-hairline">
          <span className="w-[132px] shrink-0">
            <Text tone="tertiary" size="2xs">
              {dimensionLabel}
            </Text>
          </span>
          <span className="flex-1" />
          <div className="shrink-0 flex items-center gap-3">
            <span className="w-[68px] text-right">
              <Text tone="tertiary" size="2xs">
                Token
              </Text>
            </span>
            <span className="w-[54px] text-right">
              <Text tone="tertiary" size="2xs">
                占比
              </Text>
            </span>
            <span className="w-[62px] text-right">
              <Text tone="tertiary" size="2xs">
                缓存命中
              </Text>
            </span>
            <span className="w-[76px] text-right">
              <Text tone="tertiary" size="2xs">
                成本
              </Text>
            </span>
          </div>
        </div>
      )}
    </ChartCard>
  );
};
