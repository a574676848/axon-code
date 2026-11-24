import React from 'react';
import { formatElapsedClock } from '../../conversation/format';
import { WorkingMeter } from '../ui/WorkingMeter';
import { ComposerStatusCell } from './ComposerStatusCell';

/*
 * 输入坞上方的三行提示：错误回执、操作回执、工作状态行。
 *
 * 从输入坞里拆出来只为一行一件事：这三行都是「刚才发生了什么」，
 * 与输入框本身无关，字号与语气也必须是同一套（12/20，错误取危险色）。
 */

/** 工作中的中断提示，与 TUI 的 composer dock 逐字一致。 */
const WORKING_INTERRUPT_HINT = '按 ESC 中断';

/** 一行回执：错误与说明共用同一几何，只有颜色不同。 */
export const ComposerNotice: React.FC<{ tone: 'danger' | 'secondary'; text: string }> = ({
  tone,
  text,
}) => (
  <div className="mb-2 px-1 popover-up-in">
    <span className={`text-meta ${tone === 'danger' ? 'text-danger' : 'text-text-secondary'}`}>
      {text}
    </span>
  </div>
);

export interface ComposerNoticesProps {
  /** 外部传入的错误（提交失败、会话异常）。 */
  error: string | null;
  /** 输入坞自己的错误（历史写入失败）。 */
  localError: string | null;
  /** 附件与拖放的过程说明；`danger` 时按错误配色。 */
  notice: { tone: 'info' | 'danger'; text: string } | null;
}

/**
 * 卡片上方那几行回执。
 *
 * 三行同一位置、同一字号，谁有话说谁出现；全部为空时不占高度。
 */
export const ComposerNotices: React.FC<ComposerNoticesProps> = ({
  error,
  localError,
  notice,
}) => (
  <>
    {error !== null && <ComposerNotice tone="danger" text={error} />}
    {localError !== null && <ComposerNotice tone="danger" text={localError} />}
    {notice !== null && (
      <ComposerNotice
        tone={notice.tone === 'danger' ? 'danger' : 'secondary'}
        text={notice.text}
      />
    )}
  </>
);

export interface ComposerWorkingLineProps {
  primary: string;
  elapsedMs: number | null;
  queued: number;
  /** 是否给出 ESC 中断提示；本机指令（`/mcp`）不画这一句。 */
  interruptible?: boolean;
}

/**
 * 工作状态行。
 *
 * 运行期间一直在，耗时与中断提示都写在这一行里，用户不必去别处找。
 * 与语音录音条共用 `ComposerStatusCell` 外壳：同一条状态列里两格同高同形，
 * 只是语气不同（一格在收声，一格在干活）。
 *
 * 读数按「这一行正在做的事」从左到右排：工作脉搏、主文案、耗时、排队条数，
 * 右端固定中断提示。底边的墨色光泽说明它确实在动。
 * 左端脉搏坐在一枚下沉底的小方槽里，颜色一律取墨水档：这一行是常驻读数，
 * 不叠强调色，免得比正文更抢眼。
 *
 * 内置指令复用这一行（`interruptible=false`）：它同样是「这件事正在做」，
 * 只是不能被打断，因此不承诺 ESC。
 */
export const ComposerWorkingLine: React.FC<ComposerWorkingLineProps> = ({
  primary,
  elapsedMs,
  queued,
  interruptible = true,
}) => {
  const elapsed = elapsedMs !== null && elapsedMs > 0 ? formatElapsedClock(elapsedMs) : undefined;
  return (
    <ComposerStatusCell
      role="status"
      aria-live="polite"
      className="mb-2"
      glyph={
        <span className="flex h-5 w-5 items-center justify-center rounded-sm bg-surface-sunken text-text-primary">
          <WorkingMeter />
        </span>
      }
      hint={interruptible ? WORKING_INTERRUPT_HINT : undefined}
      sweeping
    >
      <span className="shimmer-text-active min-w-0 truncate text-row font-medium text-text-primary">
        {primary}
      </span>
      {elapsed !== undefined && (
        <>
          <span aria-hidden="true" className="shrink-0 self-center h-3 w-px bg-border-hairline" />
          <span className="shrink-0 text-meta tabular-nums text-text-tertiary">{elapsed}</span>
        </>
      )}
      {queued > 0 && (
        <span className="shrink-0 text-meta tabular-nums text-text-tertiary">
          另有 {queued} 条等待注入
        </span>
      )}
    </ComposerStatusCell>
  );
};
