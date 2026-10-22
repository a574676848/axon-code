import React, { useMemo, useState } from 'react';
import { Text } from '../../ui/surfaces';
import { ChartCard } from './chartKit';
import { HEALTH_MAX_DAYS, type HealthRow } from '../usageBuckets';
import type { UsageHealthStatus } from '../../../types';
import { formatRatio } from '../usageFormat';

/*
 * 请求健康时间线：行是一天、列是一小时，格子的颜色表示那一小时的终态。
 *
 * 「没有请求的小时」与「跑成功了的小时」必须分得开，因此空格取中性槽色而不是绿色：
 * 绿格是结论，空格是「这一小时没跑」。失败与中断都算 degraded 与 failed，
 * 与大盘 summary 的成败口径同源（网关按终态判定，不看有没有配到价格）。
 *
 * 读数不用浮层而用网格下方一行固定高度的说明：这一格的信息很短，浮层会盖住
 * 相邻的格子，还让整块在悬浮时抖一下；固定高度的一行两件事都没有。
 */

/** 状态到取色的映射：与全站状态色同源。 */
const STATUS_FILL: Record<UsageHealthStatus, string> = {
  healthy: 'var(--color-ok-soft)',
  degraded: 'var(--color-gold)',
  failed: 'var(--color-danger)',
};

const STATUS_LABEL: Record<UsageHealthStatus, string> = {
  healthy: '全部成功',
  degraded: '部分失败',
  failed: '全部失败',
};

function dayLabel(ms: number): string {
  const date = new Date(ms);
  return `${date.getMonth() + 1}/${date.getDate()}`;
}

export const HealthTimeline: React.FC<{ rows: HealthRow[] }> = ({ rows }) => {
  const [hover, setHover] = useState<{ row: number; hour: number } | null>(null);

  const summary = useMemo(() => {
    let healthy = 0;
    let degraded = 0;
    let failed = 0;
    let idle = 0;
    for (const row of rows) {
      for (const cell of row.cells) {
        if (cell.status === null) idle += 1;
        else if (cell.status === 'healthy') healthy += 1;
        else if (cell.status === 'degraded') degraded += 1;
        else failed += 1;
      }
    }
    return { healthy, degraded, failed, idle };
  }, [rows]);

  const hovered = hover === null ? undefined : rows[hover.row]?.cells[hover.hour];

  return (
    <ChartCard
      title="请求健康时间线"
      description={`按小时给出的终态分布，最多回看 ${HEALTH_MAX_DAYS} 天`}
      legend={[
        { label: '全部成功', color: STATUS_FILL.healthy },
        { label: '部分失败', color: STATUS_FILL.degraded },
        { label: '全部失败', color: STATUS_FILL.failed },
        { label: '没有请求', color: 'var(--color-chart-track)' },
      ]}
      meta={
        <Text tone="secondary" size="meta">
          成功 {summary.healthy} · 异常 {summary.degraded + summary.failed} · 空闲 {summary.idle}
        </Text>
      }
      empty={rows.length === 0 ? '这一周期还没有请求记录' : undefined}
      emptyHeight={180}
      footnote="口径：成功是终态 completed，其余终态（失败、中断）计入异常；没有请求的小时是空格"
    >
      <div className="space-y-[3px] pt-1 min-h-[180px]">
        {/* 顶部小时刻度：每 6 小时标一次，24 格逐格标注会糊成一片。 */}
        <div className="flex gap-[3px] pl-[47px]" aria-hidden="true">
          {Array.from({ length: 24 }, (_, hour) => (
            <div key={hour} className="flex-1 min-w-0 text-center">
              {hour % 6 === 0 ? (
                <Text tone="tertiary" size="2xs" mono>
                  {String(hour).padStart(2, '0')}
                </Text>
              ) : null}
            </div>
          ))}
        </div>
        {rows.map((row, rowIndex) => (
          <div key={row.dayMs} className="flex items-center gap-[3px]">
            <div className="w-11 shrink-0 text-right pr-0.5">
              <Text tone="tertiary" size="2xs" mono>
                {dayLabel(row.dayMs)}
              </Text>
            </div>
            {row.cells.map((cell) => (
              <button
                key={cell.hour}
                type="button"
                tabIndex={-1}
                aria-label={`${dayLabel(row.dayMs)} ${String(cell.hour).padStart(2, '0')} 时：${
                  cell.status === null ? '没有请求' : STATUS_LABEL[cell.status]
                }`}
                onMouseEnter={() => setHover({ row: rowIndex, hour: cell.hour })}
                onMouseLeave={() =>
                  setHover((current) =>
                    current?.row === rowIndex && current.hour === cell.hour ? null : current,
                  )
                }
                className="flex-1 min-w-0 h-3 rounded-[2px] hairline border-border-hairline transition-opacity duration-100 hover:opacity-80"
                style={{
                  backgroundColor:
                    cell.status === null ? 'var(--color-chart-track)' : STATUS_FILL[cell.status],
                }}
              />
            ))}
          </div>
        ))}
        <div className="h-4 flex items-center pt-1">
          {hovered === undefined || hover === null ? (
            <Text tone="tertiary" size="meta">
              指针停在格子上读出该小时的成功率
            </Text>
          ) : (
            <Text tone="secondary" size="meta">
              {`${dayLabel(rows[hover.row].dayMs)} ${String(hover.hour).padStart(2, '0')}:00 · ${
                hovered.status === null ? '没有请求' : STATUS_LABEL[hovered.status]
              }${
                hovered.status === null || hovered.status === 'healthy'
                  ? ''
                  : ` · 成功率 ${formatRatio(hovered.successRate)}`
              }`}
            </Text>
          )}
        </div>
      </div>
    </ChartCard>
  );
};
