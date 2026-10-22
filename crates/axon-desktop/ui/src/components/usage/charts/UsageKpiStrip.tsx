import React from 'react';
import { Text } from '../../ui/surfaces';
import type { UsageOverviewReport, UsageRealtimeReport } from '../../../types';
import { costLabel, formatCount, formatPercent, formatRate, formatTokens } from '../usageFormat';

/*
 * 概览指标条：一眼读数加一条迷你走势。
 *
 * 每格的结构固定为「标签、大数、一行口径说明、迷你走势」，因此六格之间可以横向
 * 对比而不必逐格重读。走势画的是这一格的度量本身（请求、Token、成本），
 * 没有观测时那一格不画线也不留空高：缺观测必须显式可见，但不能让六格的底边参差。
 *
 * 色点已移除：序列色只用于走势本身，状态语义只走状态点与语义色，两者不互相顶替。
 */

/** 迷你走势：面积加折线，百分比坐标，因此容器多宽图就多宽。 */
export const Sparkline: React.FC<{ values: number[]; color: string; height?: number }> = ({
  values,
  color,
  height = 34,
}) => {
  if (values.length === 0) {
    return null;
  }
  if (values.length === 1) {
    // 只有一个桶时画不出趋势，画一条水平基线说明「就这一点」。
    return (
      <svg className="w-full" style={{ height }} aria-hidden="true">
        <line
          x1="0"
          x2="100%"
          y1="70%"
          y2="70%"
          style={{ stroke: color }}
          strokeWidth={1.5}
          strokeLinecap="round"
          opacity={0.55}
        />
      </svg>
    );
  }
  const max = Math.max(...values, 1);
  const step = 100 / (values.length - 1);
  const points = values.map((value, index) => {
    const x = index * step;
    // 上下各留 12% 的呼吸位，波峰不会贴到框线上。
    const y = 88 - (value / max) * 76;
    return { x, y };
  });
  const line = points.map((point) => `${point.x}%,${point.y}%`).join(' ');
  const area = `0%,100% ${line} 100%,100%`;
  return (
    <svg className="w-full" style={{ height }} preserveAspectRatio="none" aria-hidden="true">
      <polygon points={area} style={{ fill: color }} opacity={0.14} />
      <polyline
        points={line}
        style={{ stroke: color }}
        strokeWidth={1.5}
        fill="none"
        strokeLinejoin="round"
        strokeLinecap="round"
      />
    </svg>
  );
};

interface KpiCell {
  label: string;
  value: string;
  detail: string;
  color: string;
  series: number[];
}

/** 概览指标条：读数取自大盘汇总，走势取自同一份响应里的迷你序列。 */
export const UsageKpiStrip: React.FC<{
  overview: UsageOverviewReport | null;
  realtime: UsageRealtimeReport | null;
  periodLabel: string;
}> = ({ overview, realtime, periodLabel }) => {
  const summary = overview?.summary;
  const live = realtime;
  const cells: KpiCell[] = [
    {
      label: '请求总数',
      value: summary === undefined ? '无观测' : formatCount(summary.requestCount),
      detail:
        summary === undefined
          ? '这一周期还没有请求记录'
          : `成功 ${formatCount(summary.successCount)} · 失败 ${formatCount(summary.failureCount)}`,
      color: 'var(--color-chart-requests)',
      series: overview?.sparklines.requests ?? [],
    },
    {
      label: '总 Token',
      value: summary === undefined ? '无观测' : formatTokens(summary.totalTokens),
      detail:
        summary === undefined
          ? '这一周期还没有请求记录'
          : `输入 ${formatTokens(summary.totalInputTokens)} · 输出 ${formatTokens(summary.outputTokens)}`,
      color: 'var(--color-chart-input)',
      series: overview?.sparklines.tokens ?? [],
    },
    {
      label: '缓存命中率',
      value: summary === undefined ? '无观测' : formatPercent(summary.cacheHitRateBasisPoints),
      detail:
        summary === undefined
          ? '这一周期还没有请求记录'
          : `命中 ${formatTokens(summary.cacheReadInputTokens)} · 写入 ${formatTokens(summary.cacheWriteInputTokens)}`,
      color: 'var(--color-chart-cache-read)',
      series: [],
    },
    {
      label: '成功率',
      value: summary === undefined ? '无观测' : formatPercent(summary.successRateBasisPoints),
      detail:
        summary === undefined
          ? '这一周期还没有请求记录'
          : `按 ${formatCount(summary.requestCount)} 次请求的终态统计`,
      color: 'var(--color-chart-output)',
      series: [],
    },
    {
      label: '平均 RPM / TPM',
      value:
        summary === undefined
          ? '无观测'
          : `${formatRate(summary.currentRpm)} / ${formatCount(summary.currentTpm)}`,
      detail:
        live === null
          ? `${periodLabel}内的平均速率`
          : `滑窗内 ${formatRate(live.metrics.averageRpm)} / ${formatCount(Math.round(live.metrics.averageTpm))}`,
      color: 'var(--color-chart-cache-write)',
      series: [],
    },
    {
      label: '成本',
      value:
        summary === undefined
          ? '无观测'
          : costLabel(summary.totalCostMicros, summary.requestCount),
      detail:
        summary === undefined
          ? '这一周期还没有请求记录'
          : summary.totalCostMicros > 0
            ? '按已配置的价格规则计算'
            : '配好价格规则后这里给出金额',
      color: 'var(--color-chart-reasoning)',
      series:
        summary === undefined || summary.totalCostMicros <= 0 ? [] : overview?.sparklines.costs ?? [],
    },
  ];

  return (
    <div className="grid grid-cols-2 @[640px]:grid-cols-3 @[860px]:grid-cols-6 gap-2">
      {cells.map((cell) => (
        <div
          key={cell.label}
          className="min-h-[118px] min-w-0 flex flex-col rounded-md bg-surface hairline border-border-hairline px-3 pt-2.5 pb-2"
        >
          <Text tone="secondary" size="xs" truncate className="block">
            {cell.label}
          </Text>
          <Text size="base" strong className="block mt-0.5 tabular-nums">
            {cell.value}
          </Text>
          <Text tone="tertiary" size="2xs" className="block h-8 leading-4 mt-0.5 overflow-hidden">
            {cell.detail}
          </Text>
          <div className="mt-auto">
            <Sparkline values={cell.series} color={cell.color} />
          </div>
        </div>
      ))}
    </div>
  );
};
