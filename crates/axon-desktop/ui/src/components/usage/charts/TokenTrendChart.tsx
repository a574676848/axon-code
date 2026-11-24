import React, { useMemo, useState } from 'react';
import { SegmentedControl } from '../../ui/controls';
import { Text } from '../../ui/surfaces';
import {
  ChartCard,
  ChartTooltip,
  GridLines,
  PlotArea,
  TooltipRow,
  clamp,
  type LegendItem,
} from './chartKit';
import {
  axisLabel,
  bucketRangeLabel,
  classTotals,
  niceTicks,
  stackColumns,
  type TokenBucket,
  type TrendGranularity,
} from '../usageBuckets';
import { costLabel, formatCostMicros, formatCount, formatTokens } from '../usageFormat';

/*
 * 用量趋势：一张图三种读法。
 *
 * 三种度量（Token 构成、请求数、成本）的**量纲完全不同**，因此不做双轴叠加，
 * 而是同一张图切换指标：同一时刻只有一套刻度，读数不会被两条轴互相解释。
 *
 * 柱子按时间轴等宽排布，缺数据的时段是空档（不是被拉宽的邻柱）：
 * 时间轴由周期决定，不由「有几条数据」决定。
 */

/** 趋势图的指标模式。 */
export type TrendMode = 'composition' | 'requests' | 'cost';

const MODE_LABEL: Record<TrendMode, string> = {
  composition: 'Token 构成',
  requests: '请求数',
  cost: '成本',
};

interface TokenTrendChartProps {
  buckets: TokenBucket[];
  granularity: TrendGranularity;
  mode: TrendMode;
  onModeChange: (mode: TrendMode) => void;
  /** 周期名称，用于口径说明与无障碍名称。 */
  periodLabel: string;
  /** Token 总量，用于标题右侧读数。 */
  totalTokens: number;
}

export const TokenTrendChart: React.FC<TokenTrendChartProps> = ({
  buckets,
  granularity,
  mode,
  onModeChange,
  periodLabel,
  totalTokens,
}) => {
  const [hover, setHover] = useState<number | null>(null);

  const totals = useMemo(() => classTotals(buckets), [buckets]);
  const totalCostMicros = useMemo(
    () => buckets.reduce((sum, bucket) => sum + bucket.costMicros, 0),
    [buckets],
  );
  const totalRequests = useMemo(
    () => buckets.reduce((sum, bucket) => sum + bucket.requests, 0),
    [buckets],
  );
  const costPriced = totalCostMicros > 0;

  const stacks = useMemo(() => stackColumns(buckets), [buckets]);

  const metricValues = useMemo(() => {
    if (mode === 'requests') return buckets.map((bucket) => bucket.requests);
    if (mode === 'cost') return buckets.map((bucket) => bucket.costMicros / 1_000_000);
    return buckets.map((bucket) => bucket.totalTokens);
  }, [buckets, mode]);

  const maxValue = Math.max(...metricValues, 0);
  const ticks = niceTicks(maxValue);
  const scaleMax = ticks[ticks.length - 1] ?? 1;

  const formatMetric = (value: number): string => {
    if (mode === 'requests') return formatCount(value);
    if (mode === 'cost') return formatCostMicros(value * 1_000_000);
    return formatTokens(value);
  };

  const yLabels = [...ticks].reverse().map((tick) => formatMetric(tick));

  // 底部刻度最多六条：再多就挤成一片，读不出时间。
  const xLabels = useMemo(() => {
    if (buckets.length === 0) return [];
    const wanted = Math.min(6, buckets.length);
    return Array.from({ length: wanted }, (_, index) => {
      const bucketIndex = Math.round((index * (buckets.length - 1)) / Math.max(1, wanted - 1));
      return {
        label: axisLabel(buckets[bucketIndex].startMs, granularity),
        ratio: buckets.length === 1 ? 0.5 : bucketIndex / (buckets.length - 1),
      };
    });
  }, [buckets, granularity]);

  const legend: LegendItem[] =
    mode === 'composition'
      ? totals.map((item) => ({
          label: item.spec.label,
          color: item.spec.color,
          value: formatTokens(item.total),
        }))
      : [
          {
            label: MODE_LABEL[mode],
            color: mode === 'cost' ? 'var(--color-chart-reasoning)' : 'var(--color-chart-requests)',
            value: mode === 'cost' ? formatCostMicros(totalCostMicros) : formatCount(totalRequests),
          },
        ];

  const empty =
    buckets.length === 0
      ? '这一周期还没有聚合结果，Daemon 每分钟投影一次'
      : mode === 'cost' && !costPriced
        ? '所选周期内没有定价成本，配置价格规则后这里会给出金额'
        : maxValue <= 0
          ? '所选周期内没有请求记录'
          : undefined;

  const hoveredBucket = hover === null ? undefined : buckets[hover];

  return (
    <ChartCard
      title="用量趋势"
      description={`${periodLabel}内按时间分桶，缺数据的时段留空档`}
      action={
        <SegmentedControl
          label="趋势指标"
          value={mode}
          onChange={onModeChange}
          options={[
            { value: 'composition', label: MODE_LABEL.composition },
            { value: 'requests', label: MODE_LABEL.requests },
            {
              value: 'cost',
              label: MODE_LABEL.cost,
              disabled: !costPriced,
              title: costPriced ? MODE_LABEL.cost : '所选周期内没有已定价的请求',
            },
          ]}
        />
      }
      legend={empty === undefined ? legend : undefined}
      meta={
        empty === undefined ? (
          <Text tone="secondary" size="meta">
            合计 {formatTokens(totalTokens)} Token
          </Text>
        ) : undefined
      }
      empty={empty}
      emptyHeight={180}
      footnote={
        mode === 'composition'
          ? undefined
          : '同一张图切换度量，不做双轴叠加：一个时刻只有一套刻度'
      }
    >
      <PlotArea height={180} yLabels={yLabels} xLabels={xLabels}>
        <GridLines count={ticks.length - 1} />
        <svg
          className="absolute inset-0 w-full h-full"
          role="img"
          aria-label={`用量趋势，${buckets.length} 个时间桶，峰值 ${formatMetric(maxValue)}`}
        >
          {buckets.map((bucket, index) => {
            const column = 100 / buckets.length;
            const width = column * 0.64;
            const x = index * column + (column - width) / 2;
            const active = hover === index;
            if (mode === 'composition') {
              return (
                <g key={bucket.startMs} opacity={hover === null || active ? 1 : 0.55}>
                  {stacks[index].map((segment) =>
                    segment.value <= 0 ? null : (
                      <rect
                        key={segment.key}
                        x={`${x}%`}
                        width={`${width}%`}
                        y={`${100 - (segment.to / scaleMax) * 100}%`}
                        height={`${((segment.to - segment.from) / scaleMax) * 100}%`}
                        style={{ fill: segment.color }}
                      />
                    ),
                  )}
                </g>
              );
            }
            const value = metricValues[index];
            return (
              <rect
                key={bucket.startMs}
                x={`${x}%`}
                width={`${width}%`}
                y={`${100 - (value / scaleMax) * 100}%`}
                height={`${(value / scaleMax) * 100}%`}
                rx={1}
                opacity={hover === null || active ? 1 : 0.55}
                style={{
                  fill:
                    mode === 'cost' ? 'var(--color-chart-reasoning)' : 'var(--color-chart-requests)',
                }}
              />
            );
          })}
        </svg>

        {/* 命中列：整列可悬浮，读数按桶给出，指针不必压在细柱上。 */}
        <div className="absolute inset-0 flex" onMouseLeave={() => setHover(null)}>
          {buckets.map((bucket, index) => (
            <div
              key={bucket.startMs}
              className={`h-full transition-colors duration-100 ${
                hover === index ? 'bg-hover' : ''
              }`}
              style={{ width: `${100 / buckets.length}%` }}
              onMouseEnter={() => setHover(index)}
            />
          ))}
        </div>

        {hoveredBucket !== undefined && hover !== null && (
          <ChartTooltip ratio={clamp((hover + 0.5) / buckets.length, 0, 1)}>
            <TooltipRow
              label={bucketRangeLabel(hoveredBucket.startMs, granularity)}
              value={`${formatCount(hoveredBucket.requests)} 次`}
            />
            {mode === 'composition' ? (
              <>
                {totals.map((item) => (
                  <TooltipRow
                    key={item.spec.key}
                    label={item.spec.label}
                    color={item.spec.color}
                    value={formatTokens(hoveredBucket[item.spec.key])}
                  />
                ))}
                <TooltipRow label="合计" value={formatTokens(hoveredBucket.totalTokens)} />
              </>
            ) : (
              <TooltipRow
                label={MODE_LABEL[mode]}
                value={formatMetric(metricValues[hover])}
              />
            )}
          </ChartTooltip>
        )}
      </PlotArea>
      {costPriced ? null : (
        <Text tone="tertiary" size="meta" className="block mt-1.5">
          成本口径：{costLabel(totalCostMicros, totalRequests)}，Axon 不预设模型价格
        </Text>
      )}
    </ChartCard>
  );
};
