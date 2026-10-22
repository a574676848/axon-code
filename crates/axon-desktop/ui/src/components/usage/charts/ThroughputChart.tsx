import React, { useMemo, useState } from 'react';
import { Text } from '../../ui/surfaces';
import { ChartCard, ChartTooltip, GridLines, PlotArea, TooltipRow, clamp } from './chartKit';
import { niceTicks } from '../usageBuckets';
import type { UsageRealtimeReport, UsageRealtimeWindow } from '../../../types';
import { formatClock, formatCount, formatLatency } from '../usageFormat';

/*
 * 实时吞吐：滑窗内每分钟的 Token 与请求数。
 *
 * 两个量纲相差三四个数量级（Token 上千、请求个位数），因此左右各一套刻度、
 * 各自一种颜色，并在图例里写清哪个读数看哪条轴。这是全看板唯一一处双轴图，
 * 只因为这两条线天生要一起看：请求进来了多少、Token 又跑了多少。
 */

/** 平滑折线用的采样点，百分比坐标。 */
function linePoints(values: number[], max: number): string {
  const step = values.length <= 1 ? 0 : 100 / (values.length - 1);
  return values
    .map((value, index) => {
      const x = values.length === 1 ? 0 : index * step;
      const y = 92 - (value / Math.max(1, max)) * 84;
      return `${x}%,${y}%`;
    })
    .join(' ');
}

const WINDOWS: { value: UsageRealtimeWindow; label: string }[] = [
  { value: '15m', label: '15 分钟' },
  { value: '30m', label: '30 分钟' },
  { value: '60m', label: '60 分钟' },
];

export const RealtimeWindows = WINDOWS;

export const ThroughputChart: React.FC<{
  report: UsageRealtimeReport | null;
  window: UsageRealtimeWindow;
  /** 窗口切换控件：状态归容器，图只负责画。 */
  action?: React.ReactNode;
}> = ({ report, window: activeWindow, action }) => {
  const [hover, setHover] = useState<number | null>(null);

  const series = report?.throughputSeries ?? [];
  const tokens = useMemo(() => series.map((point) => point.tokenPerMin), [series]);
  const requests = useMemo(() => series.map((point) => point.requestPerMin), [series]);

  const tokenMax = Math.max(...tokens, 0);
  const requestMax = Math.max(...requests, 0);
  const ticks = niceTicks(tokenMax);
  const scaleMax = ticks[ticks.length - 1] ?? 1;

  const xLabels = useMemo(() => {
    if (series.length === 0) return [];
    const wanted = Math.min(5, series.length);
    return Array.from({ length: wanted }, (_, index) => {
      const pointIndex = Math.round((index * (series.length - 1)) / Math.max(1, wanted - 1));
      return {
        label: formatClock(series[pointIndex].timestampMs),
        ratio: series.length === 1 ? 0.5 : pointIndex / (series.length - 1),
      };
    });
  }, [series]);

  const windowLabel = WINDOWS.find((item) => item.value === activeWindow)?.label ?? activeWindow;
  const hovered = hover === null ? undefined : series[hover];

  return (
    <ChartCard
      title="实时吞吐"
      description={`最近 ${windowLabel}内每分钟的 Token 与请求数，滑窗留在 Daemon 进程内`}
      action={action}
      legend={[
        { label: 'Token / 分钟', color: 'var(--color-chart-input)' },
        { label: '请求 / 分钟', color: 'var(--color-chart-requests)' },
      ]}
      meta={
        report === null ? undefined : (
          <Text tone="secondary" size="meta">
            当前 {formatCount(report.metrics.currentTpm)} TPM · {report.metrics.currentRpm} RPM
          </Text>
        )
      }
      empty={
        report === null
          ? '实时滑窗不可用'
          : series.length === 0
            ? '滑窗内还没有采样点'
            : undefined
      }
      emptyHeight={160}
      footnote={`首位延迟当前 ${formatLatency(report?.ttftCurrentMs)}、平均 ${formatLatency(report?.ttftAverageMs)}；总耗时当前 ${formatLatency(report?.latencyCurrentMs)}、平均 ${formatLatency(report?.latencyAverageMs)}`}
    >
      <PlotArea
        height={160}
        yLabels={[...ticks].reverse().map((tick) => formatCount(tick))}
        xLabels={xLabels}
      >
        <GridLines count={Math.max(1, ticks.length - 1)} />
        <svg
          className="absolute inset-0 w-full h-full"
          role="img"
          aria-label={`实时吞吐：窗口内峰值 ${formatCount(tokenMax)} Token 每分钟、${formatCount(requestMax)} 请求每分钟`}
        >
          <polygon
            points={`0%,100% ${linePoints(tokens, scaleMax)} 100%,100%`}
            style={{ fill: 'var(--color-chart-input)' }}
            opacity={0.16}
          />
          <polyline
            points={linePoints(tokens, scaleMax)}
            style={{ stroke: 'var(--color-chart-input)' }}
            strokeWidth={1.5}
            fill="none"
            strokeLinejoin="round"
          />
          <polyline
            points={linePoints(requests, requestMax)}
            style={{ stroke: 'var(--color-chart-requests)' }}
            strokeWidth={1.5}
            fill="none"
            strokeDasharray="3 3"
            strokeLinejoin="round"
          />
        </svg>
        <div className="absolute inset-0 flex" onMouseLeave={() => setHover(null)}>
          {series.map((point, index) => (
            <div
              key={point.timestampMs}
              className={`h-full transition-colors duration-100 ${hover === index ? 'bg-hover' : ''}`}
              style={{ width: `${100 / Math.max(1, series.length)}%` }}
              onMouseEnter={() => setHover(index)}
            />
          ))}
        </div>
        {hovered !== undefined && hover !== null && (
          <ChartTooltip ratio={clamp((hover + 0.5) / series.length, 0, 1)}>
            <TooltipRow label="时刻" value={formatClock(hovered.timestampMs)} />
            <TooltipRow
              label="Token / 分钟"
              color="var(--color-chart-input)"
              value={formatCount(hovered.tokenPerMin)}
            />
            <TooltipRow
              label="请求 / 分钟"
              color="var(--color-chart-requests)"
              value={String(hovered.requestPerMin)}
            />
          </ChartTooltip>
        )}
      </PlotArea>
    </ChartCard>
  );
};
