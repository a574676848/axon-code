import React from 'react';
import { useReducedMotion } from 'motion/react';

/*
 * Space 长按唤醒的进度反馈：贴着麦克风入口的一条细进度线。
 *
 * 只推 transform（scaleX）与 opacity，都在合成层完成，不触发布局与重绘；
 * 按住时跟手填充，松手未到点时用一个短过渡退回，避免进度瞬间消失的闪烁。
 * 减弱动效下保留进度线本身，只把过渡改为瞬时——状态还在，位移归零。
 */

/** 未到点松手后，进度退回与淡出共用的时长。 */
const RETREAT_MS = 160;

export interface VoiceHoldProgressProps {
  /** 长按进度，0 到 1；0 表示未在长按。 */
  progress: number;
}

export const VoiceHoldProgress: React.FC<VoiceHoldProgressProps> = ({ progress }) => {
  const reducedMotion = Boolean(useReducedMotion());
  const fill = Number.isFinite(progress) ? Math.min(1, Math.max(0, progress)) : 0;
  /** 是否留在画面里：按住即显示，退回动画走完再隐藏。 */
  const [visible, setVisible] = React.useState(fill > 0);
  const retreatTimer = React.useRef<number | null>(null);

  React.useEffect(() => {
    if (retreatTimer.current !== null) {
      window.clearTimeout(retreatTimer.current);
      retreatTimer.current = null;
    }
    if (fill > 0) {
      setVisible(true);
      return undefined;
    }
    // 到点开麦时 phase 已切走，这几帧显示退回也无害；轻点取消则靠它平滑收口。
    const delay = reducedMotion ? 0 : RETREAT_MS;
    retreatTimer.current = window.setTimeout(() => {
      retreatTimer.current = null;
      setVisible(false);
    }, delay);
    return undefined;
  }, [fill, reducedMotion]);

  React.useEffect(
    () => () => {
      if (retreatTimer.current !== null) window.clearTimeout(retreatTimer.current);
    },
    [],
  );

  return (
    <span
      aria-hidden="true"
      data-voice-hold-progress={visible ? (fill > 0 ? 'hold' : 'retreat') : 'idle'}
      className={`pointer-events-none absolute inset-x-1 -bottom-0.5 h-0.5 overflow-hidden rounded-full bg-text-link/15 transition-opacity ${visible ? 'opacity-100' : 'opacity-0'}`}
      style={{ transitionDuration: reducedMotion ? '0ms' : '100ms' }}
    >
      <span
        className="block h-full w-full origin-left rounded-full bg-text-link"
        style={{
          transform: `scaleX(${fill})`,
          // 按住时跟手不插值；松手归零那帧才开过渡，让退回平滑而不拖慢填充。
          transition: reducedMotion ? 'none' : fill > 0 ? 'none' : `transform ${RETREAT_MS}ms ease-out`,
        }}
      />
    </span>
  );
};
