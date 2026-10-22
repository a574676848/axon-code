import React from 'react';
import { Text } from '../../ui/surfaces';
import { ChartCard } from './chartKit';
import { TOKEN_CLASSES } from '../usageBuckets';
import type { UsageRealtimeReport, UsageTokenComposition } from '../../../types';
import { formatPercent, formatTokens } from '../usageFormat';

/*
 * 滑窗内的 Token 构成：环形加逐项读数。
 *
 * 环形只承担「大概是谁占大头」，精确值一律由右侧逐项给出：环上的角度分辨不出
 * 3% 与 4% 的差别，把读数压在小扇区里等于没写。占比为 0 的项照样列出来（写 0），
 * 因此「这一项是 0」与「这一项没统计」在界面上不会混为一谈。
 */

/** 环形几何：用 stroke-dasharray 画弧，一圈周长按半径算。 */
const RADIUS = 42;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;
/** 扇区之间的留白（周长单位），避免相邻两段糊在一起。 */
const GAP = 2;

/** 五类构成的字段名：与滑窗响应的字段一一对应。 */
const COMPOSITION_FIELD: Record<(typeof TOKEN_CLASSES)[number]['key'], keyof UsageTokenComposition> =
  {
    uncachedInput: 'uncachedInputTokens',
    cacheRead: 'cacheReadTokens',
    cacheWrite: 'cacheWriteTokens',
    output: 'outputTokens',
    reasoning: 'reasoningTokens',
  };

function compositionRows(composition: UsageTokenComposition): {
  label: string;
  color: string;
  value: number;
}[] {
  return TOKEN_CLASSES.map((spec) => ({
    label: spec.label,
    color: spec.color,
    value: composition[COMPOSITION_FIELD[spec.key]],
  }));
}

export const TokenCompositionDonut: React.FC<{ report: UsageRealtimeReport | null }> = ({
  report,
}) => {
  const rows = report === null ? [] : compositionRows(report.tokenComposition);
  const total = rows.reduce((sum, row) => sum + row.value, 0);

  let offset = 0;
  const arcs = rows.map((row) => {
    const ratio = total <= 0 ? 0 : row.value / total;
    const arc = {
      ...row,
      ratio,
      dash: Math.max(0, ratio * CIRCUMFERENCE - (ratio > 0 ? GAP : 0)),
      offset,
    };
    offset += ratio * CIRCUMFERENCE;
    return arc;
  });

  return (
    <ChartCard
      title="滑窗 Token 构成"
      description="按未缓存输入、缓存命中、缓存写入、生成与推理拆分"
      empty={
        report === null
          ? '实时滑窗不可用'
          : total <= 0
            ? '滑窗内还没有 Token 消耗'
            : undefined
      }
      emptyHeight={160}
      footnote="占比在环上只作示意，精确读数看右侧逐项"
    >
      <div className="flex items-center gap-4 min-w-0 min-h-[160px]">
        <div className="shrink-0 relative" style={{ width: 108, height: 108 }}>
          <svg
            viewBox="0 0 108 108"
            className="w-full h-full"
            role="img"
            aria-label={`滑窗 Token 合计 ${formatTokens(total)}`}
          >
            <circle
              cx="54"
              cy="54"
              r={RADIUS}
              fill="none"
              stroke="var(--color-chart-track)"
              strokeWidth={12}
            />
            {arcs.map((arc) =>
              arc.value <= 0 ? null : (
                <circle
                  key={arc.label}
                  cx="54"
                  cy="54"
                  r={RADIUS}
                  fill="none"
                  stroke={arc.color}
                  strokeWidth={12}
                  strokeDasharray={`${arc.dash} ${CIRCUMFERENCE - arc.dash}`}
                  strokeDashoffset={-arc.offset}
                  transform="rotate(-90 54 54)"
                />
              ),
            )}
          </svg>
          <div className="absolute inset-0 flex flex-col items-center justify-center">
            <Text tone="tertiary" size="2xs">
              合计
            </Text>
            <Text size="row" strong mono>
              {formatTokens(total)}
            </Text>
          </div>
        </div>
        <ul className="flex-1 min-w-0 space-y-1.5">
          {rows.map((row) => (
            <li key={row.label} className="flex items-center gap-2 min-w-0">
              <span
                className="w-2 h-2 rounded-[2px] shrink-0"
                style={{ backgroundColor: row.color }}
                aria-hidden="true"
              />
              <Text tone="secondary" size="meta" truncate className="flex-1 min-w-0">
                {row.label}
              </Text>
              <Text size="meta" mono>
                {formatTokens(row.value)}
              </Text>
              <span className="w-[52px] text-right">
                <Text tone="tertiary" size="meta" mono>
                  {formatPercent(total <= 0 ? 0 : (row.value / total) * 10_000)}
                </Text>
              </span>
            </li>
          ))}
        </ul>
      </div>
    </ChartCard>
  );
};
