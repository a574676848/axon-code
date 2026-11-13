import React from 'react';

/*
 * 指标读数卡：指标条上的读数展开后共用的那一张。
 *
 * 结构固定为「图标 + 标题 …… 合计」一行、发丝线、若干「标签 …… 数值」行，
 * 因此同一屏里上下文的占用与整条会话的用量读起来是同一套排版。
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
}

export const MetricUsageCard: React.FC<{
  /** 标题前的图标。 */
  icon: React.ReactNode;
  /** 卡片标题。 */
  title: string;
  /** 右上角的合计读数。 */
  total: string;
  /** 明细行。 */
  rows: readonly MetricUsageRow[];
  /** 合计为空时的降级说明。 */
  fallback?: string;
}> = ({ icon, title, total, rows, fallback }) => (
  <>
    <div className="flex items-center justify-between gap-3">
      <span className="flex items-center gap-1.5 min-w-0 text-body font-medium text-text-primary">
        <span aria-hidden="true" className="shrink-0 text-text-secondary">
          {icon}
        </span>
        <span className="truncate">{title}</span>
      </span>
      <span className="shrink-0 text-body font-medium text-text-primary tabular-nums">{total}</span>
    </div>
    <div className="h-px bg-border-hairline" />
    {rows.length > 0 ? (
      <div className="flex flex-col gap-1.5">
        {rows.map((row) => (
          <div key={row.label} className="flex items-baseline justify-between gap-3">
            <span className="min-w-0 truncate text-row text-text-secondary">{row.label}</span>
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
