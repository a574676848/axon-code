import React, { useCallback, useRef } from 'react';
import { calculateDraggedWidth } from './widthControls';

/*
 * 正文列两侧的宽度拖拽手柄。
 *
 * 这里只有指针与几何：宽度的解析、夹紧与持久化都在视图层的 useConversationWidth。
 * 手柄因此是一个纯展示件，不会出现「手柄以为 840、正文按 741 排」这种两套读数并存的局面。
 */

/** 单侧指针捕获拖拽手柄。 */
function WidthHandle({
  side,
  onStart,
  onDrag,
  onCommit,
  onReset,
}: {
  side: 'left' | 'right';
  onStart: () => number;
  onDrag: (width: number) => void;
  onCommit: (width: number) => void;
  onReset: () => void;
}) {
  const dragging = useRef(false);
  const base = useRef(0);
  const origin = useRef(0);
  const latest = useRef(0);
  const frame = useRef<number | null>(null);

  const cancelFrame = () => {
    if (frame.current !== null) {
      cancelAnimationFrame(frame.current);
      frame.current = null;
    }
  };

  const onPointerDown = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      try {
        event.currentTarget.setPointerCapture(event.pointerId);
      } catch {
        // 忽略不支持的环境
      }
      origin.current = event.clientX;
      latest.current = event.clientX;
      base.current = onStart();
      dragging.current = true;
      event.currentTarget.setAttribute('data-dragging', 'true');
    },
    [onStart],
  );

  const onPointerMove = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!dragging.current) return;
      const rect = event.currentTarget.getBoundingClientRect();
      event.currentTarget.style.setProperty(
        '--axon-width-handle-pointer-y',
        `${event.clientY - rect.top}px`,
      );
      latest.current = event.clientX;
      if (frame.current === null) {
        frame.current = requestAnimationFrame(() => {
          frame.current = null;
          onDrag(calculateDraggedWidth(base.current, latest.current - origin.current, side));
        });
      }
    },
    [onDrag, side],
  );

  const onPointerUp = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!dragging.current) return;
      dragging.current = false;
      event.currentTarget.removeAttribute('data-dragging');
      try {
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId);
        }
      } catch {
        // 忽略
      }
      cancelFrame();
      latest.current = event.clientX;
      if (latest.current !== origin.current) {
        onCommit(calculateDraggedWidth(base.current, latest.current - origin.current, side));
      }
    },
    [onCommit, side],
  );

  const onPointerCancel = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return;
    dragging.current = false;
    event.currentTarget.removeAttribute('data-dragging');
    try {
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
    } catch {
      // 忽略
    }
    cancelFrame();
  }, []);

  const onWheel = useCallback((event: React.WheelEvent<HTMLDivElement>) => {
    const parent = event.currentTarget.parentElement;
    if (!parent || event.ctrlKey || event.deltaY === 0) return;
    const scroller = parent.querySelector<HTMLElement>('.overflow-y-auto');
    if (scroller) {
      scroller.scrollBy({ top: event.deltaY });
    }
  }, []);

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={`${side === 'left' ? '左侧' : '右侧'}正文宽度拖拽手柄`}
      title="拖拽调节正文宽度，双击恢复默认"
      data-side={side}
      className="conversation-width-handle"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onLostPointerCapture={onPointerCancel}
      onDoubleClick={onReset}
      onWheel={onWheel}
    />
  );
}

export interface ConversationWidthControlsProps {
  /** 拖拽起点：返回本次基准宽度。 */
  onStart: () => number;
  onDrag: (width: number) => void;
  onCommit: (width: number) => void;
  onReset: () => void;
}

export const ConversationWidthControls: React.FC<ConversationWidthControlsProps> = ({
  onStart,
  onDrag,
  onCommit,
  onReset,
}) => (
  <>
    <WidthHandle
      side="left"
      onStart={onStart}
      onDrag={onDrag}
      onCommit={onCommit}
      onReset={onReset}
    />
    <WidthHandle
      side="right"
      onStart={onStart}
      onDrag={onDrag}
      onCommit={onCommit}
      onReset={onReset}
    />
  </>
);
