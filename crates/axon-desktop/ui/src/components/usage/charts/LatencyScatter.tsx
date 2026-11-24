import React, { useMemo, useState } from 'react';
import { Text } from '../../ui/surfaces';
import { ChartCard, ChartTooltip, GridLines, PlotArea, TooltipRow, clamp } from './chartKit';
import { scatterGeometry, type ScatterGeometry } from '../usageBuckets';
import type { UsagePercentiles, UsageScatterPoint } from '../../../types';
import { formatCount, formatLatency, formatTokens } from '../usageFormat';

/*
 * 延迟散点：横轴首字延迟、纵轴总耗时，点面积随 Token 体量增长。
 *
 * 参考线画 p95 的两条（网关算好的分位，不在前端重算）：一眼能看出「大部分请求
 * 挤在左下角」还是「有几个长尾把平均拖走了」。零值样本（失败或未计量）不进散点，
 * 单独计数并在脚注披露：它们堆在原点会把分布读成「都很快」。
 */

const DOT_COLOR = 'var(--color-chart-input)';

export const LatencyScatter: React.FC<{
  points: UsageScatterPoint[];
  percentiles: UsagePercentiles | null;
}> = ({ points, percentiles }) => {
  const [hover, setHover] = useState<number | null>(null);
  const geometry: ScatterGeometry = useMemo(() => scatterGeometry(points), [points]);

  const p95Ttft = percentiles?.p95TtftMs ?? null;
  const p95Latency = percentiles?.p95LatencyMs ?? null;
  const xMax = geometry.xMax;
  const yMax = geometry.yMax;

  const hovered = hover === null ? undefined : geometry.dots[hover];
  const ratioOf = (dot: { x: number }) => clamp(dot.x, 0, 1);

  return (
    <ChartCard
      title="延迟散点"
      description="横轴首字延迟、纵轴总耗时，点面积随 Token 体量增长"
      legend={[
        { label: '一次请求', color: DOT_COLOR },
        { label: 'p95 参考线', color: 'var(--color-danger)' },
      ]}
      meta={
        geometry.dots.length > 0 ? (
          <Text tone="secondary" size="meta">
            样本 {geometry.dots.length}
          </Text>
        ) : undefined
      }
      empty={geometry.dots.length === 0 ? '这一周期还没有可用的延迟样本' : undefined}
      emptyHeight={180}
      footnote={
        geometry.skipped > 0
          ? `另有 ${geometry.skipped} 个零值样本（失败或未计量）未参与描点，它们会在原点堆成一团`
          : '只统计首字延迟与总耗时都有观测的请求'
      }
    >
      <PlotArea
        height={180}
        yLabels={[
          formatLatency(yMax),
          formatLatency(yMax * 0.75),
          formatLatency(yMax * 0.5),
          formatLatency(yMax * 0.25),
          '0',
        ]}
        xLabels={[
          { label: '0', ratio: 0 },
          { label: formatLatency(xMax * 0.25), ratio: 0.25 },
          { label: formatLatency(xMax * 0.5), ratio: 0.5 },
          { label: formatLatency(xMax * 0.75), ratio: 0.75 },
          { label: formatLatency(xMax), ratio: 1 },
        ]}
      >
        <GridLines count={4} />
        <svg
          className="absolute inset-0 w-full h-full"
          role="img"
          aria-label={`延迟散点：${geometry.dots.length} 个样本，最长总耗时 ${formatLatency(yMax)}`}
        >
          {p95Ttft !== null && xMax > 0 && (
            <line
              x1={`${clamp(p95Ttft / xMax, 0, 1) * 100}%`}
              x2={`${clamp(p95Ttft / xMax, 0, 1) * 100}%`}
              y1="0"
              y2="100%"
              style={{ stroke: 'var(--color-danger)' }}
              strokeWidth={1}
              strokeDasharray="4 3"
              opacity={0.7}
            />
          )}
          {p95Latency !== null && yMax > 0 && (
            <line
              x1="0"
              x2="100%"
              y1={`${(1 - clamp(p95Latency / yMax, 0, 1)) * 100}%`}
              y2={`${(1 - clamp(p95Latency / yMax, 0, 1)) * 100}%`}
              style={{ stroke: 'var(--color-danger)' }}
              strokeWidth={1}
              strokeDasharray="4 3"
              opacity={0.7}
            />
          )}
          {geometry.dots.map((dot, index) => (
            <circle
              key={index}
              cx={`${dot.x * 100}%`}
              cy={`${dot.y * 100}%`}
              r={dot.radius}
              style={{ fill: DOT_COLOR }}
              fillOpacity={hover === null || hover === index ? 0.55 : 0.28}
              stroke={DOT_COLOR}
              strokeWidth={1}
            />
          ))}
        </svg>
        {/* 命中层：每个点自带一枚透明命中圈，指针落在点上就读这一个样本。 */}
        {geometry.dots.map((dot, index) => (
          <button
            key={index}
            type="button"
            tabIndex={-1}
            aria-label={`首字延迟 ${formatLatency(dot.source.ttftMs)}，总耗时 ${formatLatency(
              dot.source.totalDurationMs,
            )}，${formatTokens(dot.source.tokens)} Token`}
            onMouseEnter={() => setHover(index)}
            onMouseLeave={() => setHover((current) => (current === index ? null : current))}
            className="absolute w-5 h-5 rounded-full -translate-x-1/2 -translate-y-1/2"
            style={{ left: `${dot.x * 100}%`, top: `${dot.y * 100}%` }}
          />
        ))}
        {hovered !== undefined && hover !== null && (
          <ChartTooltip ratio={ratioOf(hovered)}>
            <TooltipRow label="首字延迟" value={formatLatency(hovered.source.ttftMs)} />
            <TooltipRow label="总耗时" value={formatLatency(hovered.source.totalDurationMs)} />
            <TooltipRow label="Token" value={formatTokens(hovered.source.tokens)} />
            <TooltipRow label="样本序号" value={formatCount(hover + 1)} />
          </ChartTooltip>
        )}
      </PlotArea>
    </ChartCard>
  );
};
