import React, { useEffect, useState } from 'react';
import { getCountdownPhase } from '../../conversation/questions';

/*
 * 本地读秒。
 *
 * 读数配一枚表盘：盘面按量程比例退去，数字给出精确剩余。盘面是这里唯一的图形，
 * 取色沿用既有的分级（常规二级文本、告警琥珀墨水、警报危险色），因此它在一排灰色
 * 图标里第一眼就能被看到，又不引入第二套视觉语言。
 *
 * 盘面不画刻度、不加指针：它出现的位置只有 14px，多一笔就是一团糊；「时间在退」
 * 这件事由弧长承担，精度由数字承担。
 */

/** 表盘边长；与图标按钮里的 14px 图形同档。 */
const DIAL_SIZE = 14;
/** 表盘半径；四周留出描边的位置，不贴边。 */
const DIAL_RADIUS = 5.5;
/** 表盘描边宽度；与 14px 视窗下的图标同粗细。 */
const DIAL_STROKE = 1.5;
/** 表盘周长：退去比例按它换算成描边偏移。 */
const DIAL_CIRCUMFERENCE = 2 * Math.PI * DIAL_RADIUS;
/** 秒的进位基数：读数补零用。 */
const SECONDS_PER_MINUTE = 60;
/** 秒的位数下限：读数恒两位，变化时整行不横向跳动。 */
const SECONDS_PAD = 10;

export interface QuestionCountdownProps {
  /** 截止时刻毫秒时间戳。 */
  deadlineAtMs: number;
  /** 是否已挂起。 */
  suspended?: boolean;
  /** 倒计时归零回调。 */
  onTimeout?: () => void;
  className?: string;
}

/** 表盘上的读数：分:秒，秒恒两位。 */
function formatClock(ms: number): string {
  const totalSeconds = Math.max(0, Math.ceil(ms / 1000));
  const mins = Math.floor(totalSeconds / SECONDS_PER_MINUTE);
  const secs = totalSeconds % SECONDS_PER_MINUTE;
  return `${mins}:${secs < SECONDS_PAD ? '0' : ''}${secs}`;
}

/** 完整读数：读完形如「1分05秒」，给原生提示用。 */
function formatRemaining(ms: number): string {
  const totalSeconds = Math.max(0, Math.ceil(ms / 1000));
  const mins = Math.floor(totalSeconds / SECONDS_PER_MINUTE);
  const secs = totalSeconds % SECONDS_PER_MINUTE;
  if (mins > 0) {
    return `${mins}分${secs < SECONDS_PAD ? '0' : ''}${secs}秒`;
  }
  return `${secs}秒`;
}

export const QuestionCountdown: React.FC<QuestionCountdownProps> = ({
  deadlineAtMs,
  suspended = false,
  onTimeout,
  className = '',
}) => {
  const [remainingMs, setRemainingMs] = useState<number>(() =>
    Math.max(0, deadlineAtMs - Date.now()),
  );
  /*
   * 盘面量程：这道题拿到过的最大剩余时间。
   *
   * 等待窗口会按退避延长，deadline 随之上移；量程跟着抬高而不是钉在最初的一格，
   * 延长时盘面回满，读起来就是「又给了一段时间」。
   */
  const [spanMs, setSpanMs] = useState<number>(() =>
    Math.max(0, deadlineAtMs - Date.now()),
  );

  useEffect(() => {
    if (suspended) return;

    const tick = () => {
      const left = Math.max(0, deadlineAtMs - Date.now());
      setRemainingMs(left);
      setSpanMs((span) => (left > span ? left : span));
      if (left <= 0 && onTimeout) {
        onTimeout();
      }
    };

    tick();
    const timer = setInterval(tick, 250);
    return () => clearInterval(timer);
  }, [deadlineAtMs, suspended, onTimeout]);

  if (suspended) {
    return (
      <span
        className={`question-countdown question-countdown--suspended ${className}`}
        style={{ color: 'var(--color-text-secondary)' }}
      >
        已挂起
      </span>
    );
  }

  const phase = getCountdownPhase(remainingMs);
  const color =
    phase === 'danger'
      ? 'var(--color-danger)'
      : phase === 'warning'
        ? 'var(--color-gold-ink)'
        : 'var(--color-text-secondary)';
  // 弧长就是剩下的那一份；量程为 0（窗口尚未建立）时盘面留空，不假装还有时间。
  const remainingRatio = spanMs > 0 ? Math.min(1, Math.max(0, remainingMs / spanMs)) : 0;

  return (
    <span
      className={`question-countdown question-countdown--${phase} inline-flex items-center gap-1.5 ${className}`}
      style={{
        color,
        fontWeight: phase === 'danger' ? 600 : 400,
        fontVariantNumeric: 'tabular-nums',
      }}
      title={`等待剩余时间: ${formatRemaining(remainingMs)}`}
    >
      <svg
        width={DIAL_SIZE}
        height={DIAL_SIZE}
        viewBox="0 0 16 16"
        fill="none"
        aria-hidden="true"
        className="shrink-0"
      >
        {/* 底圈：整段时间的位置感，不随读数变化。 */}
        <circle
          cx="8"
          cy="8"
          r={DIAL_RADIUS}
          stroke="currentColor"
          strokeOpacity="0.2"
          strokeWidth={DIAL_STROKE}
        />
        {/*
          剩余弧：从 12 点起按余量铺开，时间退去时它从末端收回。
          只改描边偏移，不动任何几何；旋转写在图形内部，不依赖外层 transform。
        */}
        <circle
          cx="8"
          cy="8"
          r={DIAL_RADIUS}
          stroke="currentColor"
          strokeWidth={DIAL_STROKE}
          strokeLinecap="round"
          strokeDasharray={DIAL_CIRCUMFERENCE}
          strokeDashoffset={DIAL_CIRCUMFERENCE * (1 - remainingRatio)}
          transform="rotate(-90 8 8)"
        />
      </svg>
      <span className="leading-none">{formatClock(remainingMs)}</span>
    </span>
  );
};
