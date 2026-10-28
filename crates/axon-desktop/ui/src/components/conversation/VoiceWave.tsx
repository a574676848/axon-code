import React from 'react';
import { subscribeLevel } from '../../speech/levelBus';
import { onThemeChange } from '../../theme';

/*
 * 声波微动画基元：全站表达「正在发声」只用这一份实现。
 *
 * 它被输入坞麦克风、语音状态行、消息朗读按钮、顶栏自动朗读开关与音色试听行复用，
 * 因此声学反馈在全站是同一个形状。
 *
 * 为什么是 Canvas 而不是一排 span：
 * 1. **DOM 彻底解耦**：电平每秒变几十次，走 React 状态会把整块界面一起重渲染；
 *    写 span 的 transform 又会产生几十次样式层标脏。这里振幅先进一个非响应式的
 *    环形缓冲区，由 rAF 统一画一帧，React 全程只渲染一次。
 * 2. **DPR 物理锐化**：位图按 devicePixelRatio 放大，再用 ctx.setTransform 归一化
 *    逻辑坐标，因此 Retina 与 4K 屏上柱体边缘不发虚。
 * 3. **按需休眠**：静息且采样全部衰减到 0.001 以下立刻停表，待机 CPU 占用为 0；
 *    只有在录音或播放时才会重新起表。
 */

/** 每档几何：柱数、柱宽、画布高、柱间距（逻辑像素）。 */
const GEOMETRY = {
  xs: { bars: 4, width: 2, height: 10, gap: 1.5 },
  sm: { bars: 4, width: 2, height: 12, gap: 2 },
  md: { bars: 5, width: 2.5, height: 16, gap: 2 },
} as const;

/** 每根柱的固有比例，避免平均值驱动的柱体齐平。 */
const BAR_FACTORS = [0.52, 0.9, 1, 0.72, 0.86];

/** 振幅收敛到此值以下即停表：再画下去只是空转耗电。 */
const SLEEP_THRESHOLD = 0.001;

/** 环形缓冲区的采样数：按下 60 帧每秒约等于半秒的包络历史。 */
const RING_SIZE = 32;

/** 相邻柱体之间隔几个采样帧：拉开相位，波形才有推移感。 */
const BAR_STRIDE = 4;

export type VoiceWaveSize = keyof typeof GEOMETRY;

export interface VoiceWaveProps {
  /** 是否正在发声；静息时为静态低幅刻度。 */
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

/** 圆角矩形的兼容画法：老 WebView 没有 `roundRect`。 */
function fillBar(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
): void {
  const radius = Math.min(width / 2, height / 2);
  ctx.beginPath();
  if (typeof ctx.roundRect === 'function') {
    ctx.roundRect(x, y, width, height, radius);
  } else {
    ctx.moveTo(x + radius, y);
    ctx.arcTo(x + width, y, x + width, y + height, radius);
    ctx.arcTo(x + width, y + height, x, y + height, radius);
    ctx.arcTo(x, y + height, x, y, radius);
    ctx.arcTo(x, y, x + width, y, radius);
    ctx.closePath();
  }
  ctx.fill();
}

/**
 * 声波：以固定几何绘制柱体，振幅由实测电平驱动。
 *
 * 只在 `active` 为真或采样还没收敛时请求下一帧，静息后自动停表。
 * 减弱动效下不启动任何动画帧，直接画一组静态错落刻度：活跃语义还在，位移归零。
 */
export const VoiceWave: React.FC<VoiceWaveProps> = ({ active, size = 'sm', className = '' }) => {
  const geometry = GEOMETRY[size];
  const canvasRef = React.useRef<HTMLCanvasElement>(null);
  const reduced = usePrefersReducedMotion();
  /** 非响应式环形缓冲区：振幅包络只进这里，不触发任何渲染。 */
  const ringRef = React.useRef<Float32Array>(new Float32Array(RING_SIZE));
  const headRef = React.useRef(0);
  /** 目标振幅：由电平订阅写入，每个动画帧推进一格。 */
  const targetRef = React.useRef(0);
  const rafRef = React.useRef(0);
  const runningRef = React.useRef(false);
  /** 取色：`currentColor` 读出来之后缓存，主题切换时重取。 */
  const inkRef = React.useRef('#000000');
  const activeRef = React.useRef(active);
  activeRef.current = active;

  const readInk = React.useCallback(() => {
    const canvas = canvasRef.current;
    if (canvas === null) return;
    const ink = window.getComputedStyle(canvas).color;
    if (ink.length > 0) inkRef.current = ink;
  }, []);

  /** 写入一格采样，并返回当前所有采样里的峰值。 */
  const pushSample = React.useCallback((level: number) => {
    const ring = ringRef.current;
    ring[headRef.current] = Math.min(1, Math.max(0, level));
    headRef.current = (headRef.current + 1) % RING_SIZE;
    let peak = 0;
    for (let index = 0; index < ring.length; index += 1) {
      if (ring[index] > peak) peak = ring[index];
    }
    return peak;
  }, []);

  /** 按倒序读取第 offset 格之前的采样。 */
  const sampleAt = React.useCallback((offset: number) => {
    const index = (headRef.current - 1 - offset + RING_SIZE * 2) % RING_SIZE;
    return ringRef.current[index];
  }, []);

  /** 画一帧：位图按 DPR 放大，逻辑坐标因此与 CSS 像素一一对应。 */
  const draw = React.useCallback(() => {
    const canvas = canvasRef.current;
    if (canvas === null) return;
    const dpr = window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
    const logicalWidth = geometry.bars * geometry.width + (geometry.bars - 1) * geometry.gap;
    const pixelWidth = Math.max(1, Math.round(logicalWidth * dpr));
    const pixelHeight = Math.max(1, Math.round(geometry.height * dpr));
    // 只在尺寸真的变了时改位图尺寸：赋值 width 会清空画布并重建后备缓冲。
    if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
      canvas.width = pixelWidth;
      canvas.height = pixelHeight;
    }
    const ctx = canvas.getContext('2d');
    if (ctx === null) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, logicalWidth, geometry.height);
    ctx.fillStyle = inkRef.current;
    for (let index = 0; index < geometry.bars; index += 1) {
      const factor = BAR_FACTORS[index % BAR_FACTORS.length];
      // 最右一格取最新采样，向左依次取更早的一格：波形因此是「往右推进」的。
      const sample = reduced ? 0 : sampleAt((geometry.bars - 1 - index) * BAR_STRIDE);
      const scale = reduced ? 0.3 + factor * 0.6 : scaleFor(sample, factor);
      const barHeight = Math.max(1, geometry.height * scale);
      const x = index * (geometry.width + geometry.gap);
      fillBar(ctx, x, (geometry.height - barHeight) / 2, geometry.width, barHeight);
    }
  }, [geometry, reduced, sampleAt]);

  /** 起表：已经在跑就不重复请求。 */
  const startLoop = React.useCallback(() => {
    if (runningRef.current) return;
    runningRef.current = true;
    const tick = () => {
      rafRef.current = 0;
      // 单极低通推进目标振幅：上升快、回落慢，读起来像真实声音的余响。
      const target = activeRef.current ? targetRef.current : 0;
      const peak = pushSample(target);
      draw();
      if (!activeRef.current && peak < SLEEP_THRESHOLD) {
        // 静息收敛：停表休眠，不再请求下一帧。
        ringRef.current.fill(0);
        targetRef.current = 0;
        runningRef.current = false;
        return;
      }
      rafRef.current = window.requestAnimationFrame(tick);
    };
    rafRef.current = window.requestAnimationFrame(tick);
  }, [draw, pushSample]);

  /** 停表：卸载或切到减弱动效时清掉在途帧。 */
  const stopLoop = React.useCallback(() => {
    if (rafRef.current !== 0) window.cancelAnimationFrame(rafRef.current);
    rafRef.current = 0;
    runningRef.current = false;
  }, []);

  // 取色：挂载时读一次，主题切换时重读并重画。
  React.useEffect(() => {
    readInk();
    draw();
    return onThemeChange(() => {
      readInk();
      draw();
    });
  }, [draw, readInk]);

  /*
   * 主循环的启停。
   *
   * 发声时订阅电平并起表；停止发声后不立刻停表，而是让采样自然衰减，
   * 峰值收敛到 0.001 以下再由循环自己退出，因此最后那一下回落是平滑的。
   */
  React.useEffect(() => {
    if (reduced) {
      stopLoop();
      ringRef.current.fill(0);
      targetRef.current = 0;
      draw();
      return undefined;
    }
    if (!active) {
      targetRef.current = 0;
      startLoop();
      return stopLoop;
    }
    const unsubscribe = subscribeLevel((level) => {
      targetRef.current = level;
      startLoop();
    });
    startLoop();
    return unsubscribe;
  }, [active, draw, reduced, startLoop, stopLoop]);

  // 卸载兜底：组件消失之后不该还有帧在跑。
  React.useEffect(() => stopLoop, [stopLoop]);

  const logicalWidth = geometry.bars * geometry.width + (geometry.bars - 1) * geometry.gap;

  return (
    <canvas
      ref={canvasRef}
      aria-hidden="true"
      data-voice-wave={active ? 'active' : 'idle'}
      style={{ width: logicalWidth, height: geometry.height }}
      className={`inline-block shrink-0 ${active ? 'text-current' : 'text-current/45'} ${className}`}
    />
  );
};
