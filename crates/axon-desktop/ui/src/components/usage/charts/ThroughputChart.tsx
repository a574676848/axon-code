import React, { useMemo, useState } from 'react';
import { Text } from '../../ui/surfaces';
import { ChartCard, ChartTooltip, GridLines, PlotArea, TooltipRow, clamp } from './chartKit';
import { niceTicks } from '../usageBuckets';
import type { UsageRealtimeReport, UsageRealtimeWindow } from '../../../types';
import { formatClock, formatCount, formatLatency, formatTokens } from '../usageFormat';

/*
 * 实时吞吐：滑窗内每分钟的 Token 与请求数。
 *
 * 两个量纲相差三四个数量级（Token 上百万、请求个位数），因此**各占一条条带、各带一套
 * 刻度**，共用同一条时间轴与同一个悬浮读数。
 *
 * 早先的写法是把两条线塞进同一个绘图区、请求数按自己的最大值悄悄归一化，却没有画出
 * 请求侧的刻度：那样一条 6 次请求的线会和一条 2,200 万 Token 的线一样高，读者无从
 * 判断它到底是什么量级。分开画之后每条线的位置都能对着自己的刻度读。
 *
 * 数值本身按自然分钟实测（后端把 10 秒槽位合并成分钟桶，不做折算），因此这一屏的
 * 「每分钟」是真的每分钟。
 */

/** 折线用的采样点，百分比坐标。 */
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

/** 柱状条：一根柱子一个采样点，共用同一套百分比宽度。 */
function bars(values: number[], max: number): { x: number; width: number; height: number }[] {
  const column = 100 / Math.max(1, values.length);
  return values.map((value, index) => ({
    x: index * column + column * 0.15,
    width: column * 0.7,
    height: (value / Math.max(1, max)) * 100,
  }));
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

  const tokenTicks = niceTicks(Math.max(...tokens, 0), 3);
  const tokenScale = tokenTicks[tokenTicks.length - 1] ?? 1;
  const requestTicks = niceTicks(Math.max(...requests, 0), 2);
  const requestScale = requestTicks[requestTicks.length - 1] ?? 1;

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

  // 读数取「最近有数据的一分钟」：请求是零散的，用严格的本分钟读数会让这一栏
  // 在两次请求之间一直显示 0，读起来像坏了；同时把时刻写出来，读数不会被误当成此刻。
  const latestActive = useMemo(
    () =>
      [...series].reverse().find((point) => point.tokenPerMin > 0 || point.requestPerMin > 0),
    [series],
  );

  const empty =
    report === null
      ? '实时滑窗不可用'
      : series.length === 0
        ? '滑窗内还没有采样点'
        : latestActive === undefined
          ? '滑窗内还没有请求'
          : undefined;

  return (
    <ChartCard
      title="实时吞吐"
      description={`最近 ${windowLabel}内每分钟的实测 Token 与请求数，按自然分钟分桶，滑窗留在 Daemon 进程内`}
      action={action}
      legend={[
        { label: 'Token / 分钟', color: 'var(--color-chart-input)' },
        { label: '请求 / 分钟', color: 'var(--color-chart-requests)' },
      ]}
      meta={
        latestActive === undefined ? undefined : (
          <Text tone="secondary" size="meta" mono>
            最近一分钟 {formatClock(latestActive.timestampMs)} ·{' '}
            {formatTokens(latestActive.tokenPerMin)} TPM · {latestActive.requestPerMin} RPM
          </Text>
        )
      }
      empty={empty}
      emptyHeight={160}
      footnote={`首位延迟当前 ${formatLatency(report?.ttftCurrentMs)}、平均 ${formatLatency(report?.ttftAverageMs)}；总耗时当前 ${formatLatency(report?.latencyCurrentMs)}、平均 ${formatLatency(report?.latencyAverageMs)}`}
    >
      <div className="space-y-2">
        <PlotArea
          height={116}
          yLabels={[...tokenTicks].reverse().map((tick) => formatTokens(tick))}
        >
          <GridLines count={Math.max(1, tokenTicks.length - 1)} />
          <svg
            className="absolute inset-0 w-full h-full"
            role="img"
            aria-label={`实时吞吐：窗口内峰值 ${formatTokens(Math.max(...tokens, 0))} Token 每分钟、${Math.max(...requests, 0)} 请求每分钟，按自然分钟统计`}
          >
            <polygon
              points={`0%,100% ${linePoints(tokens, tokenScale)} 100%,100%`}
              style={{ fill: 'var(--color-chart-input)' }}
              opacity={0.16}
            />
            <polyline
              points={linePoints(tokens, tokenScale)}
              style={{ stroke: 'var(--color-chart-input)' }}
              strokeWidth={1.5}
              fill="none"
              strokeLinejoin="round"
            />
          </svg>
          <HoverColumns series={series} hover={hover} onHover={setHover} />
          {hovered !== undefined && hover !== null && (
            <ChartTooltip ratio={clamp((hover + 0.5) / series.length, 0, 1)}>
              <TooltipRow label="时刻" value={formatClock(hovered.timestampMs)} />
              <TooltipRow
                label="Token / 分钟"
                color="var(--color-chart-input)"
                value={formatTokens(hovered.tokenPerMin)}
              />
              <TooltipRow
                label="请求 / 分钟"
                color="var(--color-chart-requests)"
                value={String(hovered.requestPerMin)}
              />
            </ChartTooltip>
          )}
        </PlotArea>

        <PlotArea
          height={52}
          yLabels={[...requestTicks].reverse().map((tick) => formatCount(tick))}
          xLabels={xLabels}
        >
          <GridLines count={Math.max(1, requestTicks.length - 1)} />
          <svg className="absolute inset-0 w-full h-full" aria-hidden="true">
            {bars(requests, requestScale).map((bar, index) => (
              <rect
                key={series[index]?.timestampMs ?? index}
                x={`${bar.x}%`}
                width={`${bar.width}%`}
                y={`${100 - bar.height}%`}
                height={`${bar.height}%`}
                style={{ fill: 'var(--color-chart-requests)' }}
                opacity={hover === null || hover === index ? 0.85 : 0.4}
              />
            ))}
          </svg>
          <HoverColumns series={series} hover={hover} onHover={setHover} />
        </PlotArea>
      </div>
    </ChartCard>
  );
};

/** 两条条带共用的命中层：指针落在哪一分钟，两条都读到同一分钟。 */
const HoverColumns: React.FC<{
  series: { timestampMs: number }[];
  hover: number | null;
  onHover: (index: number | null) => void;
}> = ({ series, hover, onHover }) => (
  <div className="absolute inset-0 flex" onMouseLeave={() => onHover(null)}>
    {series.map((point, index) => (
      <div
        key={point.timestampMs}
        className={`h-full transition-colors duration-100 ${hover === index ? 'bg-hover' : ''}`}
        style={{ width: `${100 / Math.max(1, series.length)}%` }}
        onMouseEnter={() => onHover(index)}
      />
    ))}
  </div>
);
