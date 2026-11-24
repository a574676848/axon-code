import React from 'react';
import { turnSummaryRows, turnSummaryText } from '../../conversation/turnSummary';
import type { TranscriptCell } from '../../conversation/transcript';
import { DividerMetaRow } from './DividerMetaRow';

/*
 * 每轮收尾的指标行。
 *
 * 全项平铺：用时、Token、缓存命中、首字与速率一次给全，与 TUI 的收尾口径同序，
 * 不再折叠成「四项加一个详情按钮」。这一行本来就是给人扫一眼的，
 * 藏掉一半等于要求读者先点开再读数。
 *
 * 收口行的形态（发丝线加一行元信息）与轮后提示共用 `DividerMetaRow`。
 */

export const SummaryBlock: React.FC<{ cell: Extract<TranscriptCell, { role: 'summary' }> }> = ({
  cell,
}) => {
  const rows = React.useMemo(() => turnSummaryRows(cell), [cell]);
  if (rows.length === 0) return null;
  return (
    <DividerMetaRow title={turnSummaryText(cell)}>
      {rows.map((row) => `${row.label} ${row.value}`).join(' · ')}
    </DividerMetaRow>
  );
};
