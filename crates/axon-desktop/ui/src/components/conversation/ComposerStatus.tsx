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
function workingDetails(working: {
  elapsedMs: number | null;
  queued: number;
  interruptible: boolean;
}): string {
  const details: string[] = [];
  if (working.elapsedMs !== null && working.elapsedMs > 0) {
    details.push(formatDurationMillisCompact(working.elapsedMs));
  }
  if (working.queued > 0) {
    details.push(`已有 ${working.queued} 条等待注入`);
  }
  // 本机指令（`/mcp` 重载）中途打断只会留下一半状态，因此这一档不给中断提示。
  if (working.interruptible) details.push(WORKING_INTERRUPT_HINT);
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
 * 转圈与列表行是同一份，行高固定，底部一条不确定进度扫光说明它确实在动。
 * 内置指令复用这一行（`interruptible=false`）：它同样是「这件事正在做」，
 * 只是不能被打断，因此不承诺 ESC。
 *
 * 视觉上是一条下沉面药丸：生成中整行从画布上浮起一格，主文案取主文本色，
 * 扫光取强调色，因此「正在干活」比一行灰字更容易被看见。
 */
export const ComposerWorkingLine: React.FC<ComposerWorkingLineProps> = ({
  primary,
  elapsedMs,
  queued,
  interruptible = true,
}) => (
  <div
    className="relative mb-2 px-2 h-7 flex items-center gap-2 overflow-hidden rounded-sm bg-surface-sunken hairline border-border-hairline"
    role="status"
    aria-live="polite"
  >
    <StateDot state="ongoing" />
    <span className="min-w-0 truncate text-meta font-medium text-text-primary">{primary}</span>
    <span className="shrink-0 text-meta tabular-nums text-text-tertiary">
      （{workingDetails({ elapsedMs, queued, interruptible })}）
    </span>
    <span
      aria-hidden="true"
      className="working-sweep pointer-events-none absolute bottom-0 -left-1/3 h-[2px] w-1/3 bg-text-link/30"
    />
  </div>
);
