import React from 'react';
import {
  SPACE_HOLD_MS,
  canStartSpaceHold,
  spaceHoldProgress,
  type SpaceKeyInput,
} from '../../speech/spaceHold';

/*
 * Space 长按唤醒的接线层：只管键盘事件、按住进度与到点回调。
 *
 * 四类不误触的判定全在 spaceHold.ts，这里负责把它接到窗口事件上：捕获阶段
 * 监听，命中就消费掉这次空格，既不让页面滚动，也不让它落进草稿成为打字。
 * 松手、失焦、切后台都撤销，不留下"以为还在按"的悬停态。
 *
 * 进度一帧更新一次，只服务界面的唤醒指示；到点先清零再回调开麦，于是进度
 * 不会卡在满格，用户看到的是"转满即开"。
 */

export interface SpaceHoldOptions {
  /** 当前是否允许唤醒：入口可用且待机。 */
  armed: boolean;
  /** 输入坞是否有草稿；有草稿时空格属于打字。 */
  hasDraft: boolean;
  /** 按住满时长后触发一次（开麦）。 */
  onTrigger: () => void;
}

export interface SpaceHold {
  /** 唤醒进度，0..1；未在长按时为 0。 */
  progress: number;
}

/**
 * 焦点是否占住了空格。
 *
 * 文本框、可编辑区与按钮都把空格当自己的键：前者要打字，后者按空格即激活。
 * 这些时候空格不属于语音，长按判定直接让路。
 */
function focusOwnsSpace(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || tag === 'BUTTON') {
    return true;
  }
  const role = target.getAttribute('role');
  return role === 'textbox' || role === 'button';
}

/** 从浏览器事件取出判定所需的字段。 */
function keyInput(event: KeyboardEvent): SpaceKeyInput {
  return {
    code: event.code,
    repeat: event.repeat,
    ctrlKey: event.ctrlKey,
    metaKey: event.metaKey,
    altKey: event.altKey,
    shiftKey: event.shiftKey,
    isComposing: event.isComposing,
    defaultPrevented: event.defaultPrevented,
  };
}

export function useSpaceHold({ armed, hasDraft, onTrigger }: SpaceHoldOptions): SpaceHold {
  const [progress, setProgress] = React.useState(0);
  /** 按住起点；0 表示当前没有在按。 */
  const startedAt = React.useRef(0);
  const frame = React.useRef<number | null>(null);
  /** 监听器只读最新选项，文字一变也不必重新注册。 */
  const options = React.useRef({ armed, hasDraft, onTrigger });
  options.current = { armed, hasDraft, onTrigger };

  const cancel = React.useCallback(() => {
    if (frame.current !== null) {
      cancelAnimationFrame(frame.current);
      frame.current = null;
    }
    startedAt.current = 0;
    setProgress(0);
  }, []);

  React.useEffect(() => {
    const tick = () => {
      const start = startedAt.current;
      if (start === 0) return;
      const elapsed = performance.now() - start;
      if (elapsed >= SPACE_HOLD_MS) {
        cancel();
        options.current.onTrigger();
        return;
      }
      setProgress(spaceHoldProgress(elapsed));
      frame.current = requestAnimationFrame(tick);
    };

    const onKeyDown = (event: KeyboardEvent) => {
      // 已经在按住时，后续的 repeat 只用来维持，不重新判定。
      if (startedAt.current !== 0) return;
      const current = options.current;
      if (!current.armed) return;
      const allowed = canStartSpaceHold(keyInput(event), {
        hasDraft: current.hasDraft,
        focusedInText: focusOwnsSpace(event.target),
      });
      if (!allowed) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      startedAt.current = performance.now();
      frame.current = requestAnimationFrame(tick);
    };

    const onKeyUp = (event: KeyboardEvent) => {
      if (event.code === 'Space') cancel();
    };

    const onVisibility = () => {
      if (document.hidden) cancel();
    };

    window.addEventListener('keydown', onKeyDown, { capture: true });
    window.addEventListener('keyup', onKeyUp, { capture: true });
    window.addEventListener('blur', cancel);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('keydown', onKeyDown, { capture: true });
      window.removeEventListener('keyup', onKeyUp, { capture: true });
      window.removeEventListener('blur', cancel);
      document.removeEventListener('visibilitychange', onVisibility);
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      startedAt.current = 0;
    };
  }, [cancel]);

  return { progress };
}
