import React, { useCallback, useEffect, useRef } from 'react';
import type { MemoryRecord } from '../types';
import { formatDateTime, formatPercent } from './pageFormat';
import { memoryKindLabel } from './memoryDraft';
import { Panel, Text } from './ui/surfaces';

/** 单条记忆的固定行高，时间轮几何按它推导。 */
const ITEM_HEIGHT_PX = 96;
/** 相邻两条之间绕 X 轴旋转的角度。 */
const STEP_DEG = 15;
/** 视觉半径：由行高与步进角推导，让相邻行在中心保持等距。 */
const RADIUS_PX = Math.round(ITEM_HEIGHT_PX / (2 * Math.tan((STEP_DEG * Math.PI) / 360)));
/** 滚轮累计多少像素推进一格，避免触控板抖动误触。 */
const WHEEL_STEP_PX = 60;
/** 距末尾多少条开始预取下一页。 */
const PREFETCH_DISTANCE = 3;
/** 同时可见的最远行数。 */
const VISIBLE_DISTANCE = 4;

const prefersReducedMotion = (): boolean =>
  typeof window !== 'undefined' &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-reduced-motion: reduce)').matches;

/** 用户可见的状态文案；weak 与 strong 属于内部治理态，不对用户展示。 */
const stateNote = (state: MemoryRecord['state']): string | null => {
  if (state === 'tombstone') return '已遗忘';
  if (state === 'quarantined') return '已隔离';
  return null;
};

interface MemoryWheelProps {
  /** 当前类型下的记忆，按时间倒序。 */
  records: MemoryRecord[];
  /** 当前聚焦下标。 */
  activeIndex: number;
  /** 聚焦变化。 */
  onActiveIndexChange: (index: number) => void;
  /** 需要更多数据时回调（接近末尾触发一次）。 */
  onPrefetch: () => void;
  /** 正在加载下一页。 */
  loadingMore: boolean;
  /** 当前聚焦行的操作区；其余行不渲染按钮，避免一整列图标叠成灰块。 */
  renderActions: (record: MemoryRecord) => React.ReactNode;
}

/**
 * 记忆时间轮。
 *
 * 鼠标滚轮与键盘上下键在同一状态机上推进聚焦项；只动画 transform 与 opacity，
 * `prefers-reduced-motion` 下退化为静态列表，键盘语义保持一致。
 * 滚到首尾后不再拦截滚轮，页面可以继续滚动。
 */
export const MemoryWheel: React.FC<MemoryWheelProps> = ({
  records,
  activeIndex,
  onActiveIndexChange,
  onPrefetch,
  loadingMore,
  renderActions,
}) => {
  const reduced = useRef(prefersReducedMotion()).current;
  const wheelRef = useRef<HTMLDivElement>(null);
  const accumulatorRef = useRef(0);
  const activeIndexRef = useRef(activeIndex);
  activeIndexRef.current = activeIndex;
  const recordsLengthRef = useRef(records.length);
  recordsLengthRef.current = records.length;

  const move = useCallback(
    (delta: number) => {
      const last = Math.max(recordsLengthRef.current - 1, 0);
      const next = Math.min(Math.max(activeIndexRef.current + delta, 0), last);
      if (next === activeIndexRef.current) return false;
      activeIndexRef.current = next;
      onActiveIndexChange(next);
      return true;
    },
    [onActiveIndexChange],
  );
  const moveRef = useRef(move);
  moveRef.current = move;

  // 滚轮必须在非被动监听器里处理，否则无法阻止页面滚动。
  // 只有真的还能推进时才 preventDefault；到头之后把滚动交还页面。
  useEffect(() => {
    const node = wheelRef.current;
    if (node === null) return;
    const onWheel = (event: WheelEvent) => {
      if (event.deltaY === 0) return;
      const direction = event.deltaY > 0 ? 1 : -1;
      const canAdvance =
        direction > 0
          ? activeIndexRef.current < recordsLengthRef.current - 1
          : activeIndexRef.current > 0;
      if (!canAdvance) {
        accumulatorRef.current = 0;
        return;
      }
      event.preventDefault();
      accumulatorRef.current += event.deltaY;
      while (Math.abs(accumulatorRef.current) >= WHEEL_STEP_PX) {
        const stepDirection = accumulatorRef.current > 0 ? 1 : -1;
        accumulatorRef.current -= stepDirection * WHEEL_STEP_PX;
        if (!moveRef.current(stepDirection)) break;
      }
    };
    node.addEventListener('wheel', onWheel, { passive: false });
    return () => node.removeEventListener('wheel', onWheel);
  }, []);

  useEffect(() => {
    if (records.length === 0) return;
    if (records.length - 1 - activeIndex <= PREFETCH_DISTANCE) onPrefetch();
  }, [activeIndex, records.length, onPrefetch]);

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'PageDown') {
      event.preventDefault();
      move(event.key === 'PageDown' ? 5 : 1);
      return;
    }
    if (event.key === 'ArrowUp' || event.key === 'PageUp') {
      event.preventDefault();
      move(event.key === 'PageUp' ? -5 : -1);
      return;
    }
    if (event.key === 'Home') {
      event.preventDefault();
      onActiveIndexChange(0);
      return;
    }
    if (event.key === 'End') {
      event.preventDefault();
      onActiveIndexChange(Math.max(records.length - 1, 0));
    }
  };

  if (records.length === 0) return null;

  if (reduced) {
    return (
      <div
        className="space-y-2 outline-none"
        tabIndex={0}
        role="listbox"
        aria-label="记忆时间轴"
        onKeyDown={onKeyDown}
      >
        {records.map((record, index) => (
          <div
            key={record.memoryId}
            role="option"
            aria-selected={index === activeIndex}
            onClick={() => onActiveIndexChange(index)}
            className={`overflow-hidden rounded-md hairline cursor-pointer transition-colors duration-100 ${
              index === activeIndex
                ? 'bg-active border-border-selected'
                : 'bg-surface border-border-hairline'
            }`}
          >
            <div className="px-4 py-2">
              <WheelRow
                record={record}
                active={index === activeIndex}
                actions={index === activeIndex ? renderActions(record) : null}
              />
            </div>
          </div>
        ))}
      </div>
    );
  }

  return (
    <Panel clip className="relative">
      <div
        ref={wheelRef}
        tabIndex={0}
        role="listbox"
        aria-label="记忆时间轮，使用鼠标滚轮或上下方向键切换"
        onKeyDown={onKeyDown}
        className="relative outline-none h-[min(460px,58vh)]"
        style={{ perspective: `${RADIUS_PX * 3}px` }}
      >
        <div
          className="absolute inset-x-0 top-1/2"
          // 整圈圆柱后移一个半径：焦点行落在 z=0，缩放恒为 1，行内容不会被透视放大后裁掉。
          style={{ transformStyle: 'preserve-3d', transform: `translateZ(-${RADIUS_PX}px)` }}
        >
          {records.map((record, index) => {
            const offset = index - activeIndex;
            const distance = Math.abs(offset);
            const visible = distance <= VISIBLE_DISTANCE;
            const active = index === activeIndex;
            return (
              <div
                key={record.memoryId}
                role="option"
                aria-selected={active}
                onClick={() => onActiveIndexChange(index)}
                className="absolute inset-x-3 cursor-pointer"
                style={{
                  height: `${ITEM_HEIGHT_PX}px`,
                  transform: `translateY(-50%) rotateX(${-offset * STEP_DEG}deg) translateZ(${RADIUS_PX}px)`,
                  opacity: visible ? Math.max(0.08, 1 - distance * 0.26) : 0,
                  pointerEvents: distance <= 2 ? 'auto' : 'none',
                  transition: 'transform 240ms cubic-bezier(0.22, 1, 0.36, 1), opacity 240ms linear',
                  zIndex: 100 - distance,
                }}
              >
                <div
                  className={`h-full overflow-hidden rounded-md px-4 py-2 hairline transition-colors duration-100 ${
                    active ? 'bg-active border-border-selected' : 'bg-surface border-border-hairline'
                  }`}
                >
                  <WheelRow
                    record={record}
                    active={active}
                    actions={active ? renderActions(record) : null}
                  />
                </div>
              </div>
            );
          })}
        </div>
        {loadingMore && (
          <div className="pointer-events-none absolute inset-x-0 bottom-2 text-center">
            <Text tone="tertiary" size="xs">
              正在加载更早的记忆
            </Text>
          </div>
        )}
      </div>
    </Panel>
  );
};

interface WheelRowProps {
  record: MemoryRecord;
  active: boolean;
  /** 非聚焦行为 null，不渲染操作区。 */
  actions: React.ReactNode;
}

/** 单条记忆的展示行；只呈现用户可见事实。 */
const WheelRow: React.FC<WheelRowProps> = ({ record, active, actions }) => {
  const note = stateNote(record.state);
  const meta = [
    `置信度 ${formatPercent(record.confidenceBasisPoints)}`,
    `衰减后权重 ${formatPercent(record.decayedScoreBasisPoints)}`,
    `半衰期 ${record.halfLifeDays ?? 30} 天`,
    `最近活跃 ${formatDateTime(record.lastActiveAtMs ?? record.updatedAtMs)}`,
  ].join(' · ');
  return (
    <div className="h-full flex items-center gap-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-xs leading-4 text-text-secondary">
          <span className="shrink-0">{memoryKindLabel(record.memoryKind)}</span>
          <span className="shrink-0 text-text-tertiary">
            {record.scope === 'personal' ? '个人' : '仓库'}
          </span>
          <time
            className="ml-auto shrink-0 tabular-nums text-text-caption"
            dateTime={new Date(record.updatedAtMs).toISOString()}
          >
            {formatDateTime(record.updatedAtMs)}
          </time>
          {note !== null && <span className="shrink-0 text-gold-ink">{note}</span>}
        </div>
        <p className="mt-0.5 text-sm leading-[22px] text-text-primary break-words line-clamp-2">
          {record.memorySummary || '无摘要'}
        </p>
        {active && (
          <p className="mt-0.5 truncate text-xs leading-4 text-text-tertiary">{meta}</p>
        )}
      </div>
      {actions !== null && (
        <div className="shrink-0 flex items-center gap-0.5">{actions}</div>
      )}
    </div>
  );
};

/** 供页面复用的百分比格式化，保持展示口径一致。 */
export const formatBasisPoints = formatPercent;
