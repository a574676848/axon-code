import React from 'react';
import { formatDurationMillisCompact } from '../../conversation/format';
import { StateDot } from '../ui/surfaces';

/*
 * 输入坞上方的三行提示：错误回执、操作回执、工作状态行。
 *
 * 从输入坞里拆出来只为一行一件事：这三行都是「刚才发生了什么」，
 * 与输入框本身无关，字号与语气也必须是同一套（12/20，错误取危险色）。
 */

/** 工作中的中断提示，与 TUI 的 composer dock 逐字一致。 */
const WORKING_INTERRUPT_HINT = '按 ESC 中断';

/** 工作状态行的明细段：已耗时 · 排队条数 · 中断提示。 */
function workingDetails(working: { elapsedMs: number | null; queued: number }): string {
  const details: string[] = [];
  if (working.elapsedMs !== null && working.elapsedMs > 0) {
    details.push(formatDurationMillisCompact(working.elapsedMs));
  }
  if (working.queued > 0) {
    details.push(`已有 ${working.queued} 条等待注入`);
  }
  details.push(WORKING_INTERRUPT_HINT);
  return details.join(' · ');
}

/** 一行回执：错误与说明共用同一几何，只有颜色不同。 */
export const ComposerNotice: React.FC<{ tone: 'danger' | 'secondary'; text: string }> = ({
  tone,
  text,
}) => (
  <div className="mb-2 px-1">
    <span className={`text-meta ${tone === 'danger' ? 'text-danger' : 'text-text-secondary'}`}>
      {text}
    </span>
  </div>
);

export interface ComposerWorkingLineProps {
  primary: string;
  elapsedMs: number | null;
  queued: number;
}

/**
 * 工作状态行。
 *
 * 运行期间一直在，耗时与中断提示都写在这一行里，用户不必去别处找。
 * 转圈与列表行是同一份，行高固定，底部一条不确定进度扫光说明它确实在动。
 */
export const ComposerWorkingLine: React.FC<ComposerWorkingLineProps> = ({
  primary,
  elapsedMs,
  queued,
}) => (
  <div className="relative mb-1.5 px-1 h-6 flex items-center gap-2 overflow-hidden rounded-sm">
    <StateDot state="ongoing" />
    <span className="min-w-0 truncate text-meta text-text-secondary">{primary}</span>
    <span className="shrink-0 text-meta tabular-nums text-text-tertiary">
      （{workingDetails({ elapsedMs, queued })}）
    </span>
    <span
      aria-hidden="true"
      className="working-sweep pointer-events-none absolute bottom-0 -left-1/3 h-[2px] w-1/3 rounded-full bg-text-primary/[0.08]"
    />
  </div>
);
