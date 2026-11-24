import React, { useMemo, useState } from 'react';
import { Text } from '../../ui/surfaces';
import { ChartCard, ChartTooltip, TooltipRow } from './chartKit';
import { type CalendarCell } from '../usageBuckets';
import { formatTokens } from '../usageFormat';

/*
 * 活动日历：按周分列的方格，深浅表示当天的 Token 活跃度。
 *
 * 分级（0 至 4 档）由网关按当周期内的最大值算好，界面不重算：同一个数在概览与
 * 日历上必须是同一档，否则两处会各说各话。
 *
 * 方格用同一支蓝色令牌的透明度分档，而不是五种新颜色：一条序列在任何底色上都
 * 成立，也不会引出第二套「活跃度色板」。格子的 hover 读数按列定位，指针停在哪
 * 一列，气泡就贴在哪一列上方。
 */

const WEEKDAY_LABELS = ['日', '一', '二', '三', '四', '五', '六'];

/**
 * 方格边长：按周数反比缩放。
 *
 * 不能用 `aspect-square` 配 `flex-1`：列数少的时候每格会被拉成几十像素的大方块，
 * 一块日历就能吃掉整屏。这里给固定边长，让整块日历稳定落在 100 至 420 像素之间，
 * 周数少时格子大而疏，周数多时小而密。
 */
function cellEdge(weeks: number): number {
  return Math.max(12, Math.min(26, Math.round(420 / Math.max(1, weeks))));
}

/** 活跃度分档的填充与透明度：0 档是空槽，1 至 4 档逐档加深。 */
function cellStyle(level: number, future: boolean): React.CSSProperties {
  if (level <= 0) {
    return {
      backgroundColor: 'var(--color-chart-track)',
      opacity: future ? 0.3 : 1,
    };
  }
  return {
    backgroundColor: 'var(--color-chart-input)',
    opacity: [0, 0.42, 0.68, 0.86, 1][Math.min(4, Math.max(1, level))],
  };
}

export const ActivityHeatmap: React.FC<{
  cells: CalendarCell[][];
  /** 分级口径说明，例如「按当日 Token 总量分为 5 档」。 */
  metricLabel: string;
  /** 每格的无障碍描述前缀，例如「当天」。 */
  unitLabel?: string;
}> = ({ cells, metricLabel, unitLabel = '当天' }) => {
  const [hover, setHover] = useState<{ cell: CalendarCell; ratio: number } | null>(null);

  const stats = useMemo(() => {
    let total = 0;
    let activeDays = 0;
    for (const column of cells) {
      for (const cell of column) {
        if (cell.future) continue;
        total += cell.value;
        if (cell.value > 0) activeDays += 1;
      }
    }
    return { total, activeDays };
  }, [cells]);

  const legend = [
    { label: '较少', color: 'var(--color-chart-track)' },
    { label: '较多', color: 'var(--color-chart-input)' },
  ];

  return (
    <ChartCard
      title="活动日历"
      description={`${cells.length} 周内每天的活跃度，浅格是${unitLabel}没有请求`}
      legend={legend}
      meta={
        <Text tone="secondary" size="meta">
          {stats.activeDays} 天有请求 · {formatTokens(stats.total)} Token
        </Text>
      }
      empty={cells.length === 0 ? '这一周期还没有聚合结果' : undefined}
      emptyHeight={180}
      footnote={metricLabel}
    >
      <div className="flex gap-2 min-w-0 pt-1 min-h-[180px]">
        <div className="shrink-0 flex flex-col" style={{ gap: 3 }} aria-hidden="true">
          {WEEKDAY_LABELS.map((label, index) => (
            <div
              key={label}
              className="flex items-center justify-end"
              style={{ height: cellEdge(cells.length) }}
            >
              {index % 2 === 1 ? (
                <Text tone="tertiary" size="2xs">
                  {label}
                </Text>
              ) : null}
            </div>
          ))}
        </div>
        <div className="relative min-w-0">
          <div className="flex" style={{ gap: 3 }}>
            {cells.map((column, columnIndex) => (
              <div key={columnIndex} className="flex flex-col" style={{ gap: 3 }}>
                {column.map((cell) => (
                  <button
                    key={cell.date}
                    type="button"
                    tabIndex={-1}
                    aria-label={`${cell.date}：${cell.value > 0 ? `${formatTokens(cell.value)} Token` : '没有请求'}`}
                    onMouseEnter={() =>
                      setHover({ cell, ratio: (columnIndex + 0.5) / Math.max(1, cells.length) })
                    }
                    onMouseLeave={() =>
                      setHover((current) => (current?.cell.date === cell.date ? null : current))
                    }
                    className="rounded-[3px] hairline border-border-hairline transition-opacity duration-100"
                    style={{
                      width: cellEdge(cells.length),
                      height: cellEdge(cells.length),
                      ...cellStyle(cell.level, cell.future),
                    }}
                  />
                ))}
              </div>
            ))}
          </div>
          {hover !== null && (
            <ChartTooltip ratio={hover.ratio}>
              <TooltipRow label={hover.cell.date} value={formatTokens(hover.cell.value)} />
              <TooltipRow
                label="活跃档位"
                value={hover.cell.level <= 0 ? '无请求' : `${hover.cell.level} / 4`}
              />
            </ChartTooltip>
          )}
        </div>
      </div>
    </ChartCard>
  );
};
