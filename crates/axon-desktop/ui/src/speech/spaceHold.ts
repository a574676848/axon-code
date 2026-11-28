/*
 * Space 长按唤醒的纯判定：四类不误触的口径都收在这里，界面只做接线。
 *
 * Space 是打字最常用键，因此在输入坞有草稿、焦点在文本框、输入法组合态、
 * 带修饰键或按键重复时，一次 keydown 都不是唤醒起点。长按进度只依赖已持续的
 * 毫秒数，松手或中断回到 idle，不给"以为失灵"的悬停态。
 */

/** 唤醒需要按住的时长。 */
export const SPACE_HOLD_MS = 3_000;

/** 一次按键事件里判定所需的字段，抽象成接口便于单测与 Tauri 事件解耦。 */
export interface SpaceKeyInput {
  code: string;
  repeat: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  isComposing: boolean;
  /** 是否已被别的捕获阶段监听消费。 */
  defaultPrevented?: boolean;
}

/** 输入坞上下文：决定是否允许把空格当成唤醒而不是打字。 */
export interface SpaceHoldContext {
  /** 输入坞是否有草稿或附件。 */
  hasDraft: boolean;
  /** 焦点是否在文本输入态。 */
  focusedInText: boolean;
}

/** 这次 keydown 能否作为长按唤醒的起点。 */
export function canStartSpaceHold(key: SpaceKeyInput, ctx: SpaceHoldContext): boolean {
  return (
    key.code === 'Space' &&
    !key.repeat &&
    !key.ctrlKey &&
    !key.metaKey &&
    !key.altKey &&
    !key.shiftKey &&
    !key.isComposing &&
    key.defaultPrevented !== true &&
    !ctx.hasDraft &&
    !ctx.focusedInText
  );
}

/** 长按判定结果：未到时长、已唤醒、或已取消。 */
export type HoldOutcome = 'pending' | 'triggered' | 'cancelled';

/**
 * 长按到点后的结论。
 *
 * 只有 start 返回 true 的那一次按键才该走到这里；elapsedMs 是按住的时长。
 */
export function resolveSpaceHold(started: boolean, elapsedMs: number): HoldOutcome {
  if (!started) return 'cancelled';
  return elapsedMs >= SPACE_HOLD_MS ? 'triggered' : 'cancelled';
}

/** 唤醒进度填充比例，钳在 0 到 1；用于环形或线性进度反馈。 */
export function spaceHoldProgress(elapsedMs: number): number {
  if (!Number.isFinite(elapsedMs) || elapsedMs <= 0) return 0;
  return Math.min(1, elapsedMs / SPACE_HOLD_MS);
}

/** 进度是否足以显示：太短的轻点不画进度，避免一闪而过的抖动。 */
export function shouldShowHoldProgress(elapsedMs: number): boolean {
  return elapsedMs > 0 && spaceHoldProgress(elapsedMs) < 1;
}
