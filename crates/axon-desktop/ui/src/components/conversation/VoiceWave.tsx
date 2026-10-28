import React from 'react';
import { subscribeLevel } from '../../speech/levelBus';

/*
 * 声波微动画基元：全站表达「正在发声」只用这一份实现。
 *
 * 输入坞麦克风、语音状态行、消息朗读按钮、顶栏自动朗读开关与音色试听行
 * 都复用它，因此声学反馈在全站是同一个形状。柱体只动 transform 与 opacity；
 * 振幅由实测电平驱动（麦克风采集或朗读播放），静息时回落为一次缓慢呼吸，
 * 减弱动效下冻结为静态错落刻度。
 */

/** 柱数对应的几何：宽度越大，柱越多、越高。 */
const GEOMETRY = {
  xs: { bars: 4, width: 2, height: 10, gap: 1.5 },
  sm: { bars: 4, width: 2, height: 12, gap: 2 },
  md: { bars: 5, width: 2.5, height: 16, gap: 2 },
} as const;

/** 每根柱的固有比例，避免平均值驱动的柱体齐平。 */
const BAR_FACTORS = [0.52, 0.9, 1, 0.72, 0.86];

export type VoiceWaveSize = keyof typeof GEOMETRY;

export interface VoiceWaveProps {
  /** 是否正在发声；静息时为缓慢呼吸。 */
  active: boolean;
  /** 几何档位。 */
  size?: VoiceWaveSize;
  /** 附加类名，只用于布局与取色（取 currentColor）。 */
  className?: string;
}

/** 把电平平滑成柱体缩放；避免逐帧跳变。 */
function scaleFor(level: number, factor: number): number {
  return 0.2 + Math.min(1, Math.max(0, level)) * factor * 0.8;
}

/** 系统是否要求减少动效；变化时跟着切换。 */
function usePrefersReducedMotion(): boolean {
  const query = '(prefers-reduced-motion: reduce)';
  const subscribe = React.useCallback((onChange: () => void) => {
    if (typeof window === 'undefined' || !window.matchMedia) return () => undefined;
    const media = window.matchMedia(query);
    media.addEventListener('change', onChange);
    return () => media.removeEventListener('change', onChange);
  }, []);
  const read = React.useCallback(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return false;
    return window.matchMedia(query).matches;
  }, []);
  return React.useSyncExternalStore(subscribe, read, () => false);
}

/**
 * 声波：以固定几何渲染柱体，振幅经订阅直接写进 DOM。
 *
 * 电平每秒会变几十次，走 React 状态会把它所在的整块界面一起重渲染；
 * 因此这里只在挂载时订阅一次，之后直接改 transform。
 * 减弱动效下不再写 transform，改为固定错落的柱高，动静彻底归零。
 */
export const VoiceWave: React.FC<VoiceWaveProps> = ({ active, size = 'sm', className = '' }) => {
  const geometry = GEOMETRY[size];
  const barsRef = React.useRef<Array<HTMLSpanElement | null>>([]);
  const reduced = usePrefersReducedMotion();

  React.useEffect(() => {
    if (!active) return undefined;
    let frame = 0;
    let target = 0;
    let rendered = 0;
    const apply = (): void => {
      frame = 0;
      // 单极低通：上升快、回落慢，读起来像真实声音的余响。
      rendered = rendered + (target - rendered) * (target > rendered ? 0.55 : 0.16);
      barsRef.current.forEach((bar, index) => {
        if (bar === null) return;
        const factor = BAR_FACTORS[index % BAR_FACTORS.length];
        bar.style.transform = `scaleY(${scaleFor(rendered, factor).toFixed(3)})`;
      });
    };
    const schedule = (): void => {
      if (frame !== 0) return;
      frame = window.requestAnimationFrame(apply);
    };
    const unsubscribe = subscribeLevel((level) => {
      target = level;
      schedule();
    });
    return () => {
      unsubscribe();
      if (frame !== 0) window.cancelAnimationFrame(frame);
    };
  }, [active]);

  return (
    <span
      aria-hidden="true"
      data-voice-wave={active ? 'active' : 'idle'}
      className={`inline-flex shrink-0 items-center justify-center ${
        active ? 'text-current' : 'text-current/45'
      } ${className}`}
      style={{ height: geometry.height, gap: geometry.gap }}
    >
      {Array.from({ length: geometry.bars }, (_, index) => {
        const factor = BAR_FACTORS[index % BAR_FACTORS.length];
        return (
          <span
            key={index}
            ref={(element) => {
              barsRef.current[index] = element;
            }}
            className={`block rounded-full bg-current origin-center ${
              reduced ? '' : active ? 'transition-transform duration-75 ease-out' : 'voice-wave-idle'
            }`}
            style={{
              width: geometry.width,
              height: reduced ? Math.round(geometry.height * (0.3 + factor * 0.6)) : geometry.height,
              alignSelf: 'center',
              transform: reduced ? undefined : `scaleY(${active ? 0.24 : 0.2 + factor * 0.45})`,
            }}
          />
        );
      })}
    </span>
  );
};
