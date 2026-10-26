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
/** 滚轮推进一格对应的像素量（支持连续亚像素跟手旋转）。 */
const WHEEL_PIXELS_PER_ITEM = 76;
/** 停止滚轮输入后触发磁吸归位的静默窗口（毫秒）。 */
const SNAP_IDLE_MS = 110;
/** 距末尾多少条开始预取下一页。 */
const PREFETCH_DISTANCE = 3;
/** 同时可见的最远行数。 */
const VISIBLE_DISTANCE = 4;
/** 虚拟挂载窗口半径：只挂载中心附近卡片，保证长列表下恒定 O(1) 节点数。 */
const MOUNT_WINDOW = VISIBLE_DISTANCE + 2;

/** 临界阻尼弹簧参数：模拟机械拨轮的跟手与磁吸质感。 */
const SPRING_STIFFNESS = 280;
const SPRING_DAMPING = 32;

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

/** 根据连续浮点偏移计算单张卡片的 3D 变换与透明度。 */
function applySlotTransform(node: HTMLDivElement, offset: number): void {
  const distance = Math.abs(offset);
  const scale = 1 - Math.min(0.06, distance * 0.016);
  const opacity = distance > VISIBLE_DISTANCE + 0.6 ? 0 : Math.max(0.06, 1 - distance * 0.25);
  node.style.transform = `translateY(-50%) rotateX(${(-offset * STEP_DEG).toFixed(2)}deg) translateZ(${RADIUS_PX}px) scale(${scale.toFixed(4)})`;
  node.style.opacity = opacity.toFixed(3);
  node.style.pointerEvents = distance <= 2.2 ? 'auto' : 'none';
  node.style.zIndex = String(100 - Math.round(distance * 10));
}

/**
 * 记忆时间轮。
 *
 * 采用连续浮点角度 + 临界阻尼弹簧驱动：滚轮或触控板滑动时 1:1 实时跟手连续旋转，
 * 松手停滑后自动磁吸归位到最近刻度；`prefers-reduced-motion` 下自动降级为静态列表。
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
  const slotRefs = useRef<Map<number, HTMLDivElement>>(new Map());

  const activeIndexRef = useRef(activeIndex);
  activeIndexRef.current = activeIndex;
  const recordsLengthRef = useRef(records.length);
  recordsLengthRef.current = records.length;
  const onChangeRef = useRef(onActiveIndexChange);
  onChangeRef.current = onActiveIndexChange;

  /** 弹簧物理状态：current 为当前连续浮点下标，target 为目标浮点下标。 */
  const physicsRef = useRef({
    current: activeIndex,
    velocity: 0,
    target: activeIndex,
    rafId: 0 as number,
    lastTime: 0,
    snapTimer: null as number | null,
  });

  const paintSlots = useCallback((centerPosition: number) => {
    slotRefs.current.forEach((node, index) => {
      applySlotTransform(node, index - centerPosition);
    });
  }, []);

  /** 启动或续接 rAF 阻尼弹簧循环，静止到位后自动停表零空转。 */
  const ensureAnimationLoop = useCallback(() => {
    const state = physicsRef.current;
    if (state.rafId !== 0) return;

    const step = (now: number) => {
      const dt = state.lastTime === 0 ? 0.016 : Math.min(0.032, (now - state.lastTime) / 1000);
      state.lastTime = now;

      const displacement = state.current - state.target;
      const force = -SPRING_STIFFNESS * displacement - SPRING_DAMPING * state.velocity;
      state.velocity += force * dt;
      state.current += state.velocity * dt;

      paintSlots(state.current);

      if (Math.abs(state.target - state.current) < 0.002 && Math.abs(state.velocity) < 0.01) {
        state.current = state.target;
        state.velocity = 0;
        state.lastTime = 0;
        state.rafId = 0;
        paintSlots(state.current);
        return;
      }
      state.rafId = window.requestAnimationFrame(step);
    };

    state.lastTime = 0;
    state.rafId = window.requestAnimationFrame(step);
  }, [paintSlots]);

  /** 设定离散目标刻度（键盘、点击或筛选重置时使用）。 */
  const jumpToIndex = useCallback(
    (nextIndex: number) => {
      const last = Math.max(recordsLengthRef.current - 1, 0);
      const clamped = Math.min(Math.max(nextIndex, 0), last);
      const state = physicsRef.current;
      if (state.snapTimer !== null) {
        window.clearTimeout(state.snapTimer);
        state.snapTimer = null;
      }
      state.target = clamped;
      if (clamped !== activeIndexRef.current) {
        activeIndexRef.current = clamped;
        onChangeRef.current(clamped);
      }
      if (!reduced) ensureAnimationLoop();
    },
    [ensureAnimationLoop, reduced],
  );

  // 外部受控 activeIndex 变化时（例如切换作用域回零），同步弹簧目标。
  useEffect(() => {
    if (reduced) return;
    const state = physicsRef.current;
    const last = Math.max(records.length - 1, 0);
    const clamped = Math.min(Math.max(activeIndex, 0), last);
    if (Math.abs(state.target - clamped) > 0.49) {
      state.target = clamped;
      ensureAnimationLoop();
    }
    paintSlots(state.current);
  }, [activeIndex, records.length, ensureAnimationLoop, paintSlots, reduced]);

  // 卸载时清理 rAF 与磁吸定时器。
  useEffect(() => {
    const state = physicsRef.current;
    return () => {
      if (state.rafId !== 0) window.cancelAnimationFrame(state.rafId);
      if (state.snapTimer !== null) window.clearTimeout(state.snapTimer);
    };
  }, []);

  // 连续滚轮与触控板跟手控制 + 停滑磁吸归位。
  useEffect(() => {
    if (reduced) return undefined;
    const node = wheelRef.current;
    if (node === null) return undefined;

    const onWheel = (event: WheelEvent) => {
      if (event.deltaY === 0) return;
      const state = physicsRef.current;
      const last = Math.max(recordsLengthRef.current - 1, 0);
      const atBoundary =
        (event.deltaY > 0 && state.target >= last - 0.001 && activeIndexRef.current >= last) ||
        (event.deltaY < 0 && state.target <= 0.001 && activeIndexRef.current <= 0);
      if (atBoundary) return;

      event.preventDefault();
      // 支持像素级 deltaMode=0 与行级 deltaMode=1（鼠标滚轮）归一化。
      const deltaPx = event.deltaMode === 1 ? event.deltaY * 28 : event.deltaY;
      // 边界允许极轻微的 0.18 格阻尼橡皮筋回弹，增强机械边界感。
      const nextTarget = Math.min(
        last + 0.18,
        Math.max(-0.18, state.target + deltaPx / WHEEL_PIXELS_PER_ITEM),
      );
      state.target = nextTarget;

      const nearest = Math.min(Math.max(Math.round(nextTarget), 0), last);
      if (nearest !== activeIndexRef.current) {
        activeIndexRef.current = nearest;
        onChangeRef.current(nearest);
      }

      if (state.snapTimer !== null) window.clearTimeout(state.snapTimer);
      state.snapTimer = window.setTimeout(() => {
        state.snapTimer = null;
        const snapped = Math.min(Math.max(Math.round(state.target), 0), last);
        state.target = snapped;
        if (snapped !== activeIndexRef.current) {
          activeIndexRef.current = snapped;
          onChangeRef.current(snapped);
        }
        ensureAnimationLoop();
      }, SNAP_IDLE_MS);

      ensureAnimationLoop();
    };

    node.addEventListener('wheel', onWheel, { passive: false });
    return () => node.removeEventListener('wheel', onWheel);
  }, [ensureAnimationLoop, reduced]);

  useEffect(() => {
    if (records.length === 0) return;
    if (records.length - 1 - activeIndex <= PREFETCH_DISTANCE) onPrefetch();
  }, [activeIndex, records.length, onPrefetch]);

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'PageDown') {
      event.preventDefault();
      jumpToIndex(activeIndexRef.current + (event.key === 'PageDown' ? 5 : 1));
      return;
    }
    if (event.key === 'ArrowUp' || event.key === 'PageUp') {
      event.preventDefault();
      jumpToIndex(activeIndexRef.current - (event.key === 'PageUp' ? 5 : 1));
      return;
    }
    if (event.key === 'Home') {
      event.preventDefault();
      jumpToIndex(0);
      return;
    }
    if (event.key === 'End') {
      event.preventDefault();
      jumpToIndex(Math.max(records.length - 1, 0));
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
                ? 'bg-surface shadow-lift border-border-selected'
                : 'bg-surface border-border-hairline hover:bg-hover'
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

  const start = Math.max(0, activeIndex - MOUNT_WINDOW);
  const end = Math.min(records.length - 1, activeIndex + MOUNT_WINDOW);
  const mountedIndices: number[] = [];
  for (let i = start; i <= end; i += 1) mountedIndices.push(i);

  return (
    <Panel clip className="relative bg-surface-sunken/35">
      {/* 右上角精密刻度计：当前第几条 / 已载入总数 */}
      <div className="pointer-events-none absolute top-2.5 right-3.5 z-30 px-2 py-0.5 rounded-full bg-surface/90 hairline border-border font-mono text-2xs tabular-nums text-text-tertiary">
        {String(activeIndex + 1).padStart(2, '0')} / {String(records.length).padStart(2, '0')}
      </div>

      {/* 上下边缘光学景深雾化遮罩 */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 top-0 h-12 z-20"
        style={{ backgroundImage: 'linear-gradient(to bottom, var(--color-surface), transparent)' }}
      />
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 bottom-0 h-12 z-20"
        style={{ backgroundImage: 'linear-gradient(to top, var(--color-surface), transparent)' }}
      />

      {/* 中央静止光学检视窗准星线 */}
      <div
        aria-hidden="true"
        style={{ height: `${ITEM_HEIGHT_PX + 8}px` }}
        className="pointer-events-none absolute inset-x-2 top-1/2 -translate-y-1/2 z-0 rounded-md hairline border-border-hairline bg-surface/40"
      >
        <span className="absolute left-0 top-3 bottom-3 w-0.5 rounded-full bg-primary/70" />
      </div>

      <div
        ref={wheelRef}
        tabIndex={0}
        role="listbox"
        aria-label="记忆时间轮，使用鼠标滚轮或上下方向键切换"
        onKeyDown={onKeyDown}
        className="relative outline-none h-[min(460px,58vh)] select-none"
        style={{ perspective: `${RADIUS_PX * 3}px` }}
      >
        <div
          className="absolute inset-x-0 top-1/2"
          style={{ transformStyle: 'preserve-3d', transform: `translateZ(-${RADIUS_PX}px)` }}
        >
          {mountedIndices.map((index) => {
            const record = records[index];
            const active = index === activeIndex;
            return (
              <div
                key={record.memoryId}
                ref={(node) => {
                  if (node !== null) {
                    slotRefs.current.set(index, node);
                    applySlotTransform(node, index - physicsRef.current.current);
                  } else {
                    slotRefs.current.delete(index);
                  }
                }}
                role="option"
                aria-selected={active}
                onClick={() => jumpToIndex(index)}
                className="absolute inset-x-3.5 cursor-pointer will-change-[transform,opacity]"
                style={{ height: `${ITEM_HEIGHT_PX}px` }}
              >
                <div
                  className={`h-full overflow-hidden rounded-md px-4 py-2 hairline transition-colors duration-150 ${
                    active
                      ? 'bg-surface shadow-lift border-border-selected'
                      : 'bg-surface/90 border-border-hairline hover:border-border-strong'
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
          <div className="pointer-events-none absolute inset-x-0 bottom-2.5 z-30 text-center">
            <Text tone="tertiary" size="xs" className="shimmer-text-active">
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
          <span className="shrink-0 font-medium text-text-primary">
            {memoryKindLabel(record.memoryKind)}
          </span>
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
        <p className="mt-0.5 text-sm leading-[22px] text-text-prose break-words line-clamp-2">
          {record.memorySummary || '无摘要'}
        </p>
        {active && (
          <p className="disclosure-enter mt-0.5 truncate text-xs leading-4 text-text-tertiary">
            {meta}
          </p>
        )}
      </div>
      {actions !== null && (
        <div className="confirm-pop shrink-0 flex items-center gap-0.5">{actions}</div>
      )}
    </div>
  );
};

/** 供页面复用的百分比格式化，保持展示口径一致。 */
export const formatBasisPoints = formatPercent;
