import React from 'react';

/*
 * 指标读数卡：指标条上的读数展开后共用的那一张。
 *
 * 结构固定为「图标 + 标题 …… 合计」一行、可选的比例色带、发丝线、若干「色块 + 标签 …… 数值」行，
 * 因此同一屏里上下文的占用与整条会话的用量读起来是同一套排版。
 *
 * 色带与色块都取主题令牌（`var()` 引用 `tokens.css` 里的调色板），不写颜色字面量：
 * 两套主题各自只换调色板，卡片的几何、字号与层级不变。
 *
 * 合计一行一定有：读者先看总量，再看它由什么构成；没有合计的明细卡等于让人自己做加法。
 * 数值一律等宽数字，标签取次文本色，让右列成为一条能扫的竖轴。
 */

/** 卡片里的一行读数。 */
export interface MetricUsageRow {
  /** 行标签。 */
  label: string;
  /** 行数值（已格式化）。 */
  value: string;
  /** 行首色块的颜色；比值类读数（如命中率）不给色块。 */
  color?: string;
  /** 悬停说明。 */
  hint?: string;
}

/** 色带里的一段。 */
export interface MetricUsageBarSegment {
  /** 稳定标识。 */
  key: string;
  /** 相对标尺的宽度百分比，累加不超过 100。 */
  percent: number;
  /** 色块颜色。 */
  color: string;
}

export const MetricUsageCard: React.FC<{
  /** 标题前的图标。 */
  icon: React.ReactNode;
  /** 卡片标题，可内联强调值（如 `上下文已用 63%`）。 */
  title: React.ReactNode;
  /** 右上角的合计读数。 */
  total: string;
  /** 比例色带；为空时整条不画。 */
  bar?: readonly MetricUsageBarSegment[];
  /** 明细行。 */
  rows: readonly MetricUsageRow[];
  /** 明细为空时的降级说明。 */
  fallback?: string;
}> = ({ icon, title, total, bar, rows, fallback }) => (
  <>
    <div className="flex items-baseline justify-between gap-3">
      <span className="flex items-baseline gap-1.5 min-w-0 text-body text-text-secondary">
        <span aria-hidden="true" className="shrink-0 self-center text-text-secondary">
          {icon}
        </span>
        <span className="truncate">{title}</span>
      </span>
      <span className="shrink-0 text-body text-text-primary tabular-nums">{total}</span>
    </div>
    {bar !== undefined && bar.length > 0 && (
      <div className="h-1.5 rounded-full bg-surface-sunken overflow-hidden flex">
        {bar.map((segment) => (
          <span
            key={segment.key}
            aria-hidden="true"
            style={{ width: `${segment.percent}%`, backgroundColor: segment.color }}
            className="h-full"
          />
        ))}
      </div>
    )}
    <div className="h-px bg-border-hairline" />
    {rows.length > 0 ? (
      <div className="flex flex-col gap-1.5">
        {rows.map((row) => (
          <div key={row.label} className="flex items-center gap-2" title={row.hint}>
            {row.color !== undefined ? (
              <span
                aria-hidden="true"
                style={{ backgroundColor: row.color }}
                className="shrink-0 w-2 h-2 rounded-xs"
              />
            ) : (
              <span aria-hidden="true" className="shrink-0 w-2" />
            )}
            <span className="min-w-0 flex-1 truncate text-row text-text-secondary">{row.label}</span>
            <span className="shrink-0 text-row text-text-primary tabular-nums">{row.value}</span>
          </div>
        ))}
      </div>
    ) : (
      // 降级必须可见：没有观测就直说，不用 0 或占位行冒充。
      <span className="text-meta text-text-tertiary">{fallback ?? '暂无明细'}</span>
    )}
  </>
);
