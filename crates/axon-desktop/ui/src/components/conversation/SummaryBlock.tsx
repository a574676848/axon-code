import React from 'react';
import { ChevronRightIcon } from '../../icons';
import { turnSummaryRows, turnSummaryText } from '../../conversation/turnSummary';
import type { TranscriptCell } from '../../conversation/transcript';

/*
 * 每轮收尾的指标行。
 *
 * 收起态只给三项（用时 · Token · 速率）：这是读者真正会停下来看一眼的量。
 * 展开后仍是 TUI 的全项同序（入、缓存命中、出、推理、命中率、首字），
 * 一项不少、顺序不变，只是不再默认铺在正文末尾当成调试输出。
 *
 * 分隔不用贯穿横线：上方 8px 留白加一条 0.5px 发丝线收口即可，
 * 读数与下一轮正文之间仍是留白。
 */

/** 收起态显示的指标；其余项收进「详情」。 */
const PRIMARY_LABELS = new Set(['用时', 'Token', '速率']);

export const SummaryBlock: React.FC<{ cell: Extract<TranscriptCell, { role: 'summary' }> }> = ({
  cell,
}) => {
  const [open, setOpen] = React.useState(false);
  const rows = React.useMemo(() => turnSummaryRows(cell), [cell]);
  if (rows.length === 0) return null;
  const full = turnSummaryText(cell);
  const primaryRows = rows.filter((row) => PRIMARY_LABELS.has(row.label));
  const collapsed = primaryRows.map((row) => `${row.label} ${row.value}`).join(' · ');
  const expandable = rows.length > primaryRows.length;
  return (
    <div className="w-full pt-2">
      <div className="h-hairline bg-border-hairline" role="separator" />
      <div className="mt-2 flex items-center gap-3">
        <span
          className="min-w-0 flex-1 text-meta tabular-nums text-text-secondary"
          title={full}
        >
          {open || !expandable ? full : collapsed}
        </span>
        {expandable && (
          <button
            type="button"
            aria-expanded={open}
            onClick={() => setOpen((value) => !value)}
            className="shrink-0 h-6 inline-flex items-center gap-0.5 rounded-sm px-1 text-meta text-text-tertiary transition-colors duration-100 hover:bg-hover hover:text-text-primary"
          >
            {open ? '收起' : '详情'}
            <ChevronRightIcon
              size={12}
              className={`transition-transform duration-150 ease-out ${open ? 'rotate-90' : ''}`}
            />
          </button>
        )}
      </div>
    </div>
  );
};
