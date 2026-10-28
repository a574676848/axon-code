import React from 'react';
import { turnSummaryRows, turnSummaryText } from '../../conversation/turnSummary';
import type { TranscriptCell } from '../../conversation/transcript';

/*
 * 每轮收尾的指标行。
 *
 * 全项平铺：用时、Token、缓存命中、首字与速率一次给全，与 TUI 的收尾口径同序，
 * 不再折叠成「四项加一个详情按钮」。这一行本来就是给人扫一眼的，
 * 藏掉一半等于要求读者先点开再读数。
 *
 * 分隔不用贯穿横线：上方留白加一条 0.5px 发丝线收口即可，
 * 读数与下一轮正文之间仍是留白。
 */

export const SummaryBlock: React.FC<{ cell: Extract<TranscriptCell, { role: 'summary' }> }> = ({
  cell,
}) => {
  const rows = React.useMemo(() => turnSummaryRows(cell), [cell]);
  if (rows.length === 0) return null;
  return (
    <div className="w-full pt-2">
      <div className="h-hairline bg-border-hairline" role="separator" />
      <div
        className="mt-2 text-meta leading-5 tabular-nums text-text-secondary"
        title={turnSummaryText(cell)}
      >
        {rows.map((row) => `${row.label} ${row.value}`).join(' · ')}
      </div>
    </div>
  );
};
