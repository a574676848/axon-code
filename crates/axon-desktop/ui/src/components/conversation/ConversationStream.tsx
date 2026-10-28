import React from 'react';
import { ArrowUpIcon, ChevronDownIcon, SpinnerIcon } from '../../icons';
import type { TurnState } from '../../conversation/transcript';
import { Text } from '../ui/surfaces';
import { MessageRail } from './MessageRail';
import { TurnBlock } from './TranscriptTurn';
import { turnWindow } from './turnWindow';
import { useConversationActions } from './actions';
import { failureBanner } from '../../conversation/failureBanner';

/*
 * 对话流：定宽居中、贴底跟随、按窗口托管渲染。
 *
 * 贴底判定用 IntersectionObserver 观察底部哨兵，并把观察根的下边界外扩 80px，
 * 正好等价于设计系统「距底超过 80px 即解除锁定」的规则，全程不监听滚动事件。
 * 长会话只挂载末尾窗口内的单元，窗口按轮次切：未变化的轮次整块跳过重建，
 * 一次增量帧的代价只落在真正变化的那一轮上。
 */

/** 一次最多挂载的单元数。 */
const RENDER_WINDOW = 400;
/** 解除贴底锁定的距底阈值。 */
const UNLOCK_DISTANCE_PX = 80;
/**
 * 参与原生虚拟化的历史轮次数。
 *
 * 只把「末尾 15 轮之外」的历史挂上 `content-visibility: auto`：正在阅读的尾部
 * 永远逐块真实渲染，因此不会出现「刚滚到的段落先空一块」的观感。
 */
const MEMOIZED_TURNS = 15;
/** 换屏入场的位移、时长与曲线；与设计系统的通用缓动同值。 */
const ENTRANCE_SHIFT_PX = 8;
const ENTRANCE_MS = 200;
const ENTRANCE_EASING = 'cubic-bezier(0.2, 0, 0, 1)';

/**
 * 这一轮是否已经在正文里给出过结论（失败卡、收尾提示都算）。
 */
function turnConclusionExplained(turn: TurnState): boolean {
  return turn.cells.some(
    (cell) =>
      cell.role === 'failure' ||
      (cell.role === 'notice' &&
        (cell.id.startsWith('turn-failed:') ||
          cell.id.startsWith('turn-interrupted:') ||
          cell.id.endsWith(':error'))),
  );
}

export interface ConversationStreamProps {
  turns: TurnState[];
  /** 递增即要求定位到最新消息：发送消息是用户的显式动作，必须看到自己刚发的那条。 */
  followToken: number;
  /**
   * 这一屏属于哪条会话。
   *
   * 变化即换了一屏正文：落点与入场都跟着重置。调用方刻意不挂 `key`：重挂载会把
   * 滚动容器一起重建，切换时先丢一次版面再重建一次，那正是顿挫的来源。
   */
  sessionKey: string | null;
  /**
   * 子代理车道的失败归属：这里的失败不是「重新发送这条消息」能解决的。
   *
   * 缺省即主会话车道；给出时横幅改成父轮归因文案。回上层会话走头部既有的
   * 「返回会话」入口，横幅不再重复给动作。
   */
  subagentLane?: { interrupted: boolean };
  /**
   * 只读会话（目标执行会话）：没有输入坞，也没有上层会话。
   *
   * 与 `subagentLane` 互斥，缺省即主会话车道。给出时失败结论只说「后台已停止推进」，
   * 不给「重新发送」这类在这个视图里不存在的动作。
   */
  readOnly?: boolean;
  /**
   * 更早的历史是否还没取完。
   *
   * 历史按「一次一条」向前翻：这里只表达「还有更早的可以取」，取完按钮即消失。
   */
  hasOlder?: boolean;
  /** 更早那一条是否在途。 */
  loadingOlder?: boolean;
  /** 取更早的一条。 */
  onLoadOlder?: () => void;
  /**
   * 前置计数。
   *
   * 更早的一页接到时间线头部会让内容整体下移；这个计数一变，正文区就把滚动位置
   * 补偿同样的高度差，读者眼前的这一段不会跳走。
   */
  prependToken?: number;
}

export const ConversationStream: React.FC<ConversationStreamProps> = ({
  turns,
  followToken,
  sessionKey,
  subagentLane,
  readOnly = false,
  hasOlder = false,
  loadingOlder = false,
  onLoadOlder,
  prependToken = 0,
}) => {
  const scrollRef = React.useRef<HTMLDivElement>(null);
  const sentinelRef = React.useRef<HTMLDivElement>(null);
  const contentRef = React.useRef<HTMLDivElement>(null);
  /** 每轮首个单元的节点登记表，供消息导航条定位与高亮。 */
  const anchors = React.useRef<Map<string, HTMLElement>>(new Map());
  const [atBottom, setAtBottom] = React.useState(true);
  const actions = useConversationActions();

  /**
   * 把正文滚到底。
   *
   * 只改自己的 `scrollTop`，不用 `scrollIntoView`：后者是全局滚动指令，
   * 会连带滚动祖先滚动容器，而且每次都会带出一段可感知的位置动画。
   */
  const scrollToBottom = React.useCallback(() => {
    const root = scrollRef.current;
    if (root === null) return;
    root.scrollTop = root.scrollHeight;
  }, []);

  /**
   * 是否已经贴底（1px 容差）。
   *
   * 跟随滚动必须是幂等的：`ResizeObserver` 会被我们自己造成的布局变化再次触发
   * （滚动条出现/消失就会改变内容宽度），没有这个判断就会连着跳好几次。
   */
  const isPinnedToBottom = React.useCallback(() => {
    const root = scrollRef.current;
    if (root === null) return true;
    return root.scrollHeight - root.scrollTop - root.clientHeight <= 1;
  }, []);

  React.useEffect(() => {
    const sentinel = sentinelRef.current;
    const root = scrollRef.current;
    if (sentinel === null || root === null) return undefined;
    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[0];
        if (entry !== undefined) setAtBottom(entry.isIntersecting);
      },
      { root, threshold: 0, rootMargin: `0px 0px ${UNLOCK_DISTANCE_PX}px 0px` },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, []);

  /*
   * 换了一屏正文即回到最新：新会话不继承上一会话的阅读位置。
   *
   * 落点在提交之前定好，第一帧就在底部，因此不会有可见的跳位；锚点表由各自的
   * 引用回调随节点卸载自行清理，这里不整体清空（那会连刚登记的新锚点一起抹掉）。
   */
  React.useLayoutEffect(() => {
    setAtBottom(true);
  }, [sessionKey]);

  /**
   * 登记轮次锚点。
   *
   * 回调必须是稳定引用：它作为 ref 落在每一轮的块上，每轮换一次新的函数就等于
   * 每轮重挂一次 ref，长会话下这本身就是一笔可观的分配。
   */
  const registerAnchor = React.useCallback((turnId: string, node: HTMLElement | null) => {
    if (node === null) {
      anchors.current.delete(turnId);
      return;
    }
    anchors.current.set(turnId, node);
  }, []);

  const { turns: windowedTurns, hiddenCells } = React.useMemo(
    () => turnWindow(turns, RENDER_WINDOW),
    [turns],
  );

  const openChange = React.useCallback(
    (turnId: string, turnNumber: number, index: number) => {
      actions?.openChanges(turnId, turnNumber, index);
    },
    [actions],
  );

  const { hidden, lastTurn } = React.useMemo(() => {
    return {
      hidden: hiddenCells,
      lastTurn: turns.length > 0 ? turns[turns.length - 1] : null,
    };
  }, [hiddenCells, turns]);

  /*
   * 一轮只留一条结论：中断与失败的结论已经在正文里（「本轮已由用户中断。」「轮次失败 · …」），
   * 这里只在正文没有结论时补一句话；文案随车道不同，子代理车道给的是父轮归因。
   */
  const conclusion =
    lastTurn !== null &&
    !turnConclusionExplained(lastTurn) &&
    (lastTurn.status === 'rejected' || lastTurn.status === 'failed')
      ? failureBanner({
          lane:
            readOnly === true
              ? 'readonly'
              : subagentLane === undefined
                ? 'primary'
                : 'subagent',
          status: lastTurn.status,
          interrupted: subagentLane?.interrupted,
        })
      : null;
  const lastCell = lastTurn?.cells[lastTurn.cells.length - 1];
  const contentRevision =
    lastCell === undefined
      ? ''
      : lastCell.role === 'assistant' || lastCell.role === 'reasoning'
        ? lastCell.text.length
        : lastCell.role === 'tool'
          ? `${lastCell.summary.length}:${lastCell.output.length}`
          : '';

  // 仅在用户仍处于底部时自动跟随，避免打断主动上翻的阅读。
  React.useLayoutEffect(() => {
    if (!atBottom || isPinnedToBottom()) return;
    scrollToBottom();
  }, [atBottom, isPinnedToBottom, scrollToBottom, turns, windowedTurns.length, lastTurn?.cells.length, contentRevision]);

  // 展开/收起会改变 DOM 高度但不会改变投影数组；监听内容尺寸保证仍在底部时继续跟随。
  React.useEffect(() => {
    const content = contentRef.current;
    const root = scrollRef.current;
    if (content === null || root === null || typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(() => {
      if (!atBottom || isPinnedToBottom()) return;
      scrollToBottom();
    });
    observer.observe(content);
    /*
     * 视口本身也要观察：切换会话会让指标条换一组读数，指标条高度一变，贴底的视口
     * 与底部就差出几个像素，而内容尺寸没有任何变化。此前只有内容被观察，因此这一下
     * 差值要等下一次内容变化才被抹平，读起来就是一次细微的跳位。
     */
    observer.observe(root);
    return () => observer.disconnect();
  }, [atBottom, isPinnedToBottom, scrollToBottom]);

  // 发送消息时无条件回到最新：这是用户自己的动作，不该被「正在翻历史」挡住。
  React.useEffect(() => {
    if (followToken === 0) return;
    setAtBottom(true);
    scrollToBottom();
  }, [followToken, scrollToBottom]);

  /** 上一次交出的那一屏：首次挂载不做入场，开屏不该自己动。 */
  const entranceKeyRef = React.useRef<string | null>(null);

  /*
   * 换屏入场：交出正文时内容轻微上浮。
   *
   * 位移只走 transform，且只作用于滚动区里的内容列：贴底时这一列的上下边界都在
   * 视口之外，位移不会在边缘露出缝隙。减弱动效下不做位移。
   */
  React.useLayoutEffect(() => {
    const column = contentRef.current;
    const previous = entranceKeyRef.current;
    entranceKeyRef.current = sessionKey;
    if (column === null || previous === null || previous === sessionKey) return;
    if (
      typeof window.matchMedia === 'function' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches
    ) {
      return;
    }
    column.animate([{ transform: `translateY(${ENTRANCE_SHIFT_PX}px)` }, { transform: 'translateY(0)' }], {
      duration: ENTRANCE_MS,
      easing: ENTRANCE_EASING,
    });
  }, [sessionKey]);

  /** 上一次绘制后的内容高度；前置一页时用它算出位移量。 */
  const lastScrollHeightRef = React.useRef(0);
  /** 上一次已经补偿过的前置计数。 */
  const compensatedPrependRef = React.useRef(prependToken);

  /*
   * 记录内容高度。
   *
   * 放在被动 effect 里：此时浏览器已经算过布局，读 scrollHeight 不再触发一次
   * 同步布局；若改到绘制前读，正文每次渲染都要多付一次强制重排的代价。
   */
  React.useEffect(() => {
    const root = scrollRef.current;
    if (root === null) return;
    lastScrollHeightRef.current = root.scrollHeight;
  });

  /*
   * 前置更早的一页后补偿滚动位置。
   *
   * 内容接在头部会让整体下移；把 scrollTop 增加同样的高度差，读者眼前的这一段
   * 就停在原处。只在计数变化时做，因此正常渲染不会多付一次重排。
   */
  React.useLayoutEffect(() => {
    const root = scrollRef.current;
    if (root === null) return;
    if (compensatedPrependRef.current === prependToken) return;
    compensatedPrependRef.current = prependToken;
    const delta = root.scrollHeight - lastScrollHeightRef.current;
    if (delta > 0) root.scrollTop += delta;
  }, [prependToken]);

  return (
    <div className="relative flex-1 min-h-0">
      {/*
        正文区是自己的一块滚动容器，与输入坞是同级兄弟而不是叠在一起：
        输入坞浮在滚动内容之上时，正文会从输入框背面穿过去，边界感全无。
        两轴都显式声明：只写 overflow-y 时另一轴会被计算成 auto，
        于是任何一点横向溢出都会在底部长出横向滚动条，横向必须显式裁掉。
      */}
      <div
        ref={scrollRef}
        className="absolute inset-0 overflow-y-auto overflow-x-hidden px-5 pt-4 pb-3 select-text"
        /*
         * 原生滚动锚定：流式追加时由浏览器自己把视口钉住，
         * 因此不需要任何滚动监听，也不会出现「内容长高、读的位置被顶走」。
         */
        style={{ overflowAnchor: 'auto' }}
      >
        <div ref={contentRef} className="w-full max-w-chat mx-auto flex flex-col gap-2.5">
          {hasOlder && (
            /*
             * 加载更早：与下面「更早的 N 条内容已折叠」同一套语言。
             *
             * 原先是一枚独立的描边胶囊，浮在正文顶部像一张贴纸；改成两侧发丝线夹住的
             * 无边框文字按钮之后，它是这条时间线的一部分，而不是压在上面的控件。
             * 文字取三级色、悬浮才转主色，因此静止时不会与正文抢注意力。
             */
            <div className="flex items-center gap-2 pt-1">
              <span aria-hidden="true" className="flex-1 h-hairline bg-border-hairline" />
              <button
                type="button"
                onClick={onLoadOlder}
                disabled={loadingOlder}
                className="btn-tactile h-7 px-2.5 inline-flex items-center gap-1.5 rounded-full text-meta text-text-tertiary hover:text-text-primary hover:bg-hover disabled:cursor-wait disabled:opacity-60"
              >
                {loadingOlder ? <SpinnerIcon size={13} /> : <ArrowUpIcon size={13} />}
                {loadingOlder ? '正在加载更早的会话' : '加载更早'}
              </button>
              <span aria-hidden="true" className="flex-1 h-hairline bg-border-hairline" />
            </div>
          )}
          {hidden > 0 && (
            <div className="flex items-center gap-2">
              <span className="flex-1 h-hairline bg-border-hairline" />
              <Text tone="tertiary" size="meta">
                更早的 {hidden} 条内容已折叠
              </Text>
              <span className="flex-1 h-hairline bg-border-hairline" />
            </div>
          )}
          {windowedTurns.map(({ turn, index, hiddenCells: turnHidden }) => (
            <TurnBlock
              key={turn.id}
              turn={turn}
              turnNumber={index + 1}
              hiddenCells={turnHidden}
              onAnchor={registerAnchor}
              onOpenChange={openChange}
              memoized={index < windowedTurns.length - MEMOIZED_TURNS}
            />
          ))}

          {conclusion !== null && (
            <Text tone="danger" size="meta" className="block">
              {conclusion.text}
            </Text>
          )}

          <div ref={sentinelRef} className="h-px shrink-0 pointer-events-none" />
        </div>
      </div>

      {/*
        回到最新固定在输入坞正上方，并右对齐到与输入坞同宽的正文列：
        它表达的是「视口不在底部」，位置必须恒定，不能随浮动偏移飘在正文中间。
        外层放行指针事件，只有按钮本身可点，因此它压住的那几行照常可以交互。
      */}
      {!atBottom && (
        <div className="absolute bottom-2.5 left-0 right-0 z-20 flex justify-center px-5 pointer-events-none">
          <div className="w-full max-w-chat flex justify-end">
            <button
              type="button"
              onClick={() => {
                setAtBottom(true);
                scrollToBottom();
              }}
              className="popover-up-in pointer-events-auto h-7 px-3 inline-flex items-center gap-1 rounded-full hairline border-border-strong bg-surface/92 backdrop-blur-xs shadow-lift text-meta font-medium text-text-secondary transition-transform duration-100 active:scale-95 hover:bg-surface hover:text-text-primary"
            >
              <ChevronDownIcon size={13} />
              回到最新
            </button>
          </div>
        </div>
      )}

      <MessageRail turns={turns} scrollRef={scrollRef} anchors={anchors} atBottom={atBottom}
        onNavigate={() => setAtBottom(false)} />
    </div>
  );
};
