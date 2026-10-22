/*
 * 轮次收尾指标的派生：全项清单与分割线文案。
 *
 * 与 TUI 的 turn summary 逐项同序同词：用时、Token 总量、入、缓存命中、出、推理、
 * 命中率，之后才是首字与速率；没有的量一律不出现，绝不用占位值凑数。
 *
 * 分割线上直接给全项（TUI 也是全项），窄列下允许折行但不截断：少一项就等于没给，
 * 想看细节的用户没有第二个地方可看。
 */

import {
  formatBasisPointsPercent,
  formatDecimalCount,
  formatDurationMillisCompact,
  formatTokensPerSecondX10,
} from './format';
import { cacheHitRateBasisPoints, totalTokens, type TranscriptCell } from './transcript';

/** 轮次收尾指标的一项：标签与读数。 */
export interface TurnSummaryRow {
  label: string;
  value: string;
}

/** 轮次收尾指标的完整清单。 */
export function turnSummaryRows(
  cell: Extract<TranscriptCell, { role: 'summary' }>,
): TurnSummaryRow[] {
  const rows: TurnSummaryRow[] = [];
  if (cell.durationMs !== null && cell.durationMs > 0) {
    rows.push({ label: '用时', value: formatDurationMillisCompact(cell.durationMs) });
  }
  if (cell.usage !== null) {
    rows.push({ label: 'Token', value: formatDecimalCount(totalTokens(cell.usage)) });
    rows.push({ label: '入', value: formatDecimalCount(cell.usage.inputTokens) });
    rows.push({ label: '缓存命中', value: formatDecimalCount(cell.usage.cacheReadInputTokens) });
    rows.push({ label: '出', value: formatDecimalCount(cell.usage.outputTokens) });
    rows.push({ label: '推理', value: formatDecimalCount(cell.usage.reasoningOutputTokens) });
    rows.push({ label: '命中率', value: formatBasisPointsPercent(cacheHitRateBasisPoints(cell.usage)) });
  }
  if (cell.firstTokenAvgMs !== null) {
    rows.push({ label: '首字', value: formatDurationMillisCompact(cell.firstTokenAvgMs) });
  }
  if (cell.generationTpsX10 !== null) {
    rows.push({ label: '速率', value: formatTokensPerSecondX10(cell.generationTpsX10) });
  }
  return rows;
}

/** 分割线文案：全项按 TUI 同序同词拼接；没有指标时为空串。 */
export function turnSummaryText(cell: Extract<TranscriptCell, { role: 'summary' }>): string {
  return turnSummaryRows(cell)
    .map((row) => `${row.label} ${row.value}`)
    .join(' · ');
}
