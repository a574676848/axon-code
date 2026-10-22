import React from 'react';
import { Text } from '../../ui/surfaces';
import { ChartCard } from './chartKit';
import { ladderRows, niceTicks } from '../usageBuckets';
import type { UsagePercentiles } from '../../../types';
import { formatLatency } from '../usageFormat';

/*
 * 延迟阶梯：同一分位上的首字延迟与总耗时并排。
 *
 * 两个指标共用一套毫秒刻度（横条长度可直接互比），因此「等首字等了三秒、
 * 之后又跑了两分钟」这一件事在一条线上同时读得出来。首字延迟与总耗时用两种
 * 颜色的区分，且逐条写数值：读延迟的人看的是数字，不是色差。
 */

const TTFT_COLOR = 'var(--color-chart-output)';
const TOTAL_COLOR = 'var(--color-chart-input)';

export const LatencyLadder: React.FC<{ percentiles: UsagePercentiles | null }> = ({
  percentiles,
}) => {
  const rows = percentiles === null ? [] : ladderRows(percentiles);
  const values = rows.flatMap((row) => [row.ttftMs ?? 0, row.latencyMs ?? 0]);
  const max = Math.max(...values, 0);
  const ticks = niceTicks(max, 3);
  const scaleMax = ticks[ticks.length - 1] ?? 1;
  const hasObservation = max > 0;

  return (
    <ChartCard
      title="延迟分位阶梯"
      description="首字延迟与总耗时按分位并排，横条长度可直接互比"
      legend={[
        { label: '首字延迟', color: TTFT_COLOR },
        { label: '总耗时', color: TOTAL_COLOR },
      ]}
      meta={
        hasObservation ? (
          <Text tone="secondary" size="meta">
            峰值 {formatLatency(max)}
          </Text>
        ) : undefined
      }
      empty={!hasObservation ? '这一周期还没有可用的延迟观测' : undefined}
      emptyHeight={180}
      footnote="分位由网关在服务端按下限样本量计算；没有观测的分位如实写「无观测」，不补零"
    >
      <div className="space-y-1.5 min-h-[180px]">
        {rows.map((row) => (
          <div key={row.label} className="flex items-center gap-3 min-w-0">
            <span className="w-11 shrink-0">
              <Text tone="secondary" size="meta" mono>
                {row.label}
              </Text>
            </span>
            <div className="flex-1 min-w-0 space-y-1">
              <div className="h-3 rounded-[2px]" style={{ backgroundColor: 'var(--color-chart-track)' }}>
                <div
                  className="bar-grow h-full rounded-[2px]"
                  style={{
                    width: `${row.ttftMs === null ? 0 : Math.max(1, (row.ttftMs / scaleMax) * 100)}%`,
                    backgroundColor: TTFT_COLOR,
                  }}
                />
              </div>
              <div className="h-3 rounded-[2px]" style={{ backgroundColor: 'var(--color-chart-track)' }}>
                <div
                  className="bar-grow h-full rounded-[2px]"
                  style={{
                    width: `${
                      row.latencyMs === null ? 0 : Math.max(1, (row.latencyMs / scaleMax) * 100)
                    }%`,
                    backgroundColor: TOTAL_COLOR,
                  }}
                />
              </div>
            </div>
            <div className="shrink-0 w-[128px] text-right space-y-1">
              <Text size="meta" className="block">
                {formatLatency(row.ttftMs)}
              </Text>
              <Text size="meta" className="block">
                {formatLatency(row.latencyMs)}
              </Text>
            </div>
          </div>
        ))}
        {hasObservation && (
          <div className="flex items-center gap-3 pt-1">
            <span className="w-11 shrink-0" />
            <div className="flex-1 min-w-0 flex justify-between">
              {ticks.map((tick) => (
                <Text key={tick} tone="tertiary" size="2xs" mono>
                  {formatLatency(tick)}
                </Text>
              ))}
            </div>
            <span className="shrink-0 w-[128px]" />
          </div>
        )}
      </div>
    </ChartCard>
  );
};
