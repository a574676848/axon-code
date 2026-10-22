import React from 'react';
import type { TurnState } from '../../conversation/transcript';
import { Z_CONTENT_RAIL } from './chatChrome';

/*
 * 消息导航条：贴在正文列右侧预留的那条窄沟里，一条刻度对应一轮对话，
 * 当前视口所在的那一轮加长加深，点击刻度直接定位到该轮。
 *
 * 沟宽由正文列决定：列宽是 `--axon-chat-user-width`，两侧各留 88px 安全区，
 * 导航条因此永远不压在正文上，也永远在正文列的右侧边缘之外。
 *
 * 命中区 24×24（此前 3px 高、几乎点不中）；刻度本身仍是 2px 的一条短线，
 * 视觉上不抢注意力，指向起来却不费劲。
 *
 * 位置同步用 IntersectionObserver 观察每一轮的首个单元，观察根的上边界收成一条窄带：
 * 只有跨过这条带的单元才算「当前」，因此不需要监听滚动事件（门禁亦禁止）。
 */

/** 导航条允许占用的最大高度比例。 */
const MAX_HEIGHT_RATIO = 0.6;
/** 顶部窄带的高度比例；落在带内的那一轮即为当前轮。 */
const ACTIVE_BAND_TOP_RATIO = 0.08;
const ACTIVE_BAND_HEIGHT_RATIO = 0.2;

/** 一轮的定位锚点。 */
export interface TurnAnchor {
  /** 轮次标识。 */
  turnId: string;
  /** 该轮首单元的 DOM 节点。 */
  element: HTMLElement | null;
}

export interface MessageRailProps {
  turns: TurnState[];
  /** 滚动容器。 */
  scrollRef: React.RefObject<HTMLDivElement | null>;
  /** 各轮首个单元的节点登记表。 */
  anchors: React.RefObject<Map<string, HTMLElement>>;
  atBottom: boolean;
  onNavigate: () => void;
}

export const MessageRail: React.FC<MessageRailProps> = ({ turns, scrollRef, anchors, atBottom, onNavigate }) => {
  const [activeId, setActiveId] = React.useState<string | null>(null);
  const [hoveredId, setHoveredId] = React.useState<string | null>(null);

  /** 只有带用户消息的轮次才值得导航。 */
  const navigable = React.useMemo(
    () =>
      turns.filter((turn) =>
        turn.cells.some((cell) => cell.role === 'user' && cell.text.trim().length > 0),
      ),
    [turns],
  );

  React.useEffect(() => {
    const root = scrollRef.current;
    const registry = anchors.current;
    if (root === null || registry === null || navigable.length === 0) return undefined;

    const rootRect = root.getBoundingClientRect();
    const top = rootRect.height * ACTIVE_BAND_TOP_RATIO;
    const height = rootRect.height * ACTIVE_BAND_HEIGHT_RATIO;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const turnId = (entry.target as HTMLElement).dataset.turnId;
          if (turnId !== undefined) setActiveId(turnId);
        }
      },
      { root, rootMargin: `-${top}px 0px -${rootRect.height - top - height}px 0px`, threshold: 0 },
    );
    for (const turn of navigable) {
      const element = registry.get(turn.id);
      if (element !== undefined) observer.observe(element);
    }
    return () => observer.disconnect();
  }, [anchors, navigable, scrollRef]);

  const jumpTo = React.useCallback(
    (turnId: string) => {
      onNavigate();
      setActiveId(turnId);
      anchors.current?.get(turnId)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
    },
    [anchors, onNavigate],
  );

  if (navigable.length < 2) return null;

  return (
    <div
      role="navigation"
      aria-label="消息导航"
      className={`${Z_CONTENT_RAIL} absolute right-2 top-1/2 -translate-y-1/2 flex flex-col items-center overflow-y-auto no-scrollbar py-1`}
      style={{ maxHeight: `${MAX_HEIGHT_RATIO * 100}%` }}
    >
      {navigable.map((turn) => {
        const active = turn.id === (atBottom ? navigable[navigable.length - 1]?.id : activeId);
        const hovered = turn.id === hoveredId;
        const preview = turn.cells.find((cell) => cell.role === 'user');
        const label = preview !== undefined && preview.role === 'user' ? preview.text : '';
        return (
          <button
            key={turn.id}
            type="button"
            title={label}
            aria-label={`定位到：${label.slice(0, 40)}`}
            aria-current={active ? 'location' : undefined}
            onClick={() => jumpTo(turn.id)}
            onMouseEnter={() => setHoveredId(turn.id)}
            onMouseLeave={() => setHoveredId(null)}
            className="group shrink-0 flex items-center justify-center w-6 h-6 cursor-pointer"
          >
            <span
              className={`block h-0.5 rounded-full transition-colors duration-150 ${
                active
                  ? 'w-4 bg-text-primary'
                  : hovered
                    ? 'w-3 bg-text-secondary'
                    : 'w-2 bg-border-strong'
              }`}
            />
          </button>
        );
      })}
    </div>
  );
};
