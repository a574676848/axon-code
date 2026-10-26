import React, { useCallback, useEffect, useState } from 'react';
import { createPortal } from 'react-dom';

/*
 * 行悬浮卡：指针停在工作区或会话行上时，在行右侧展开一张信息卡。
 *
 * 几何与观感逐项对齐既有工作台产品：
 * 卡片位于行的右缘外 8px、顶端与行对齐，底部按 8px 边距夹取，
 * 因此靠近窗口底部时会整体上移而不会被截断。
 * 卡片本身不接收指针事件，指针离开行即收起，不会因为卡片抢焦点而闪断。
 *
 * 两张皮共用同一套几何与开关行为：`inverse` 是行信息用的反色深卡，
 * `surface` 是浅色指标浮层。差别只在配色与内边距，位置算法只此一份。
 */

/** 卡片与行右缘的间距。 */
const ANCHOR_GAP = 8;
/** 卡片与窗口边缘的最小间距。 */
const VIEWPORT_MARGIN = 8;
/** 反色信息卡的固定宽度。 */
const CARD_WIDTH = 244;
/** 浅色指标卡的固定宽度：够放下「标签 + 数值」两列。 */
export const SURFACE_CARD_WIDTH = 268;

/** 卡片皮肤。 */
export type HoverCardTone = 'inverse' | 'surface';

interface HoverCardProps {
  /** 卡片内容；为空时不展开。 */
  content: React.ReactNode;
  /** 被悬浮的行。 */
  children: React.ReactNode;
  /** 该行是否禁用悬浮卡。 */
  disabled?: boolean;
  /** 卡片皮肤。 */
  tone?: HoverCardTone;
  /** 卡片宽度。 */
  width?: number;
}

export const HoverCard: React.FC<HoverCardProps> = ({
  content,
  children,
  disabled = false,
  tone = 'inverse',
  width = CARD_WIDTH,
}) => {
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const [height, setHeight] = useState(0);

  const close = useCallback(() => setPosition(null), []);

  /** 指针进入行时测量行位置并按规则摆放卡片。 */
  const open = useCallback(
    (event: React.PointerEvent<HTMLDivElement> | React.FocusEvent<HTMLDivElement>) => {
      if (disabled) return;
      // 包裹层是 display: contents，本身没有盒子，矩形测量必须落到真正的行元素上，
      // 否则量到的是空矩形，卡片会被摆到窗口左上角。
      const row = event.currentTarget.firstElementChild;
      if (row === null) return;
      const rect = row.getBoundingClientRect();
      setPosition({ left: rect.right + ANCHOR_GAP, top: rect.top });
    },
    [disabled],
  );

  // 窗口尺寸变化时收起，避免卡片停在过时的位置。
  useEffect(() => {
    if (position === null) return;
    window.addEventListener('resize', close);
    return () => window.removeEventListener('resize', close);
  }, [position, close]);

  return (
    <>
      <div
        className="contents"
        onPointerEnter={open}
        onPointerLeave={close}
        onPointerCancel={close}
        onFocus={open}
        onBlur={close}
      >
        {children}
      </div>
      {position !== null &&
        createPortal(
          <div
            aria-hidden="true"
            ref={(node) => {
              if (node !== null && height !== node.offsetHeight) setHeight(node.offsetHeight);
            }}
            style={{
              left: Math.max(VIEWPORT_MARGIN, Math.min(position.left, window.innerWidth - width - VIEWPORT_MARGIN)),
              top: Math.max(VIEWPORT_MARGIN, Math.min(position.top, window.innerHeight - height - VIEWPORT_MARGIN)),
              width,
              maxHeight: window.innerHeight - VIEWPORT_MARGIN * 2,
            }}
            className={`fixed z-[100] box-border rounded-md shadow-panel pointer-events-none overflow-y-auto ${
              tone === 'inverse'
                ? 'px-4 py-3 bg-inverse hairline border-border-strong'
                : 'px-3.5 py-3 bg-surface hairline border-border-muted'
            }`}
          >
            <div className={tone === 'inverse' ? 'flex flex-col gap-2' : 'flex flex-col gap-2.5'}>
              {content}
            </div>
          </div>,
          document.body,
        )}
    </>
  );
};

/** 卡片标题：白色，与行标题同字号。 */
export const HoverCardTitle: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <span className="text-sm leading-5 text-white break-words">{children}</span>
);

/** 卡片次要行：浅灰，用于路径与时间。 */
export const HoverCardLine: React.FC<{ children: React.ReactNode; mono?: boolean }> = ({
  children,
  mono = false,
}) => (
  <span className={`text-xs leading-4 text-inverse-secondary break-all ${mono ? 'font-mono' : ''}`}>
    {children}
  </span>
);

/** 卡片状态行：图标加文字。 */
export const HoverCardStatus: React.FC<{
  icon?: React.ReactNode;
  children: React.ReactNode;
}> = ({ icon, children }) => (
  <span className="flex items-center gap-1.5 text-xs leading-4 text-inverse-secondary">
    {icon}
    {children}
  </span>
);
