/*
 * 车道历史的分页与展开。
 *
 * 两件事分开走：
 *
 * - **翻页只取索引**。一页 Interaction 一次到手，更早的直接按索引立成折叠行
 *   （见 `historySkeletonTurn`），一条都不回放：长会话的历史是几十万帧，
 *   逐条回放等于把翻页变成几十次串行读帧。
 * - **展开才回放**。点开某一条折叠行时，才在独立投影里回放它的帧，排空后把
 *   算好的单元整段换回主投影。
 *
 * 展开之所以必须走独立投影：帧的归属规则是「最后一条尚未收口的轮次」，在主投影
 * 里回放历史会把帧记到最新的轮次上。这里因此自持一份临时投影与它自己的订阅标识
 * 集合，回放期间两路帧按订阅标识分流。
 *
 * 编排层只通过四个接缝使用它：`take`（取下一个要回放的标识）、`finish`（排空收口）、
 * `routeFrames`（帧分流）、`applyAction`（按当前模式派发）。
 */

import { useCallback, useMemo, useRef, useState } from 'react';
import * as ipc from '../ipc';
import {
  INITIAL_CONVERSATION,
  applyFrames,
  attachmentsFromPaths,
  conversationReducer,
  historySkeletonTurn,
  type AttachmentRef,
  type ConversationAction,
  type ConversationState,
} from './transcript';
import type { SessionTokenBaseline } from './sessionUsage';
import type { ConversationStreamEvent } from './wire';
import type { LaneEffectiveModel } from './viewConfig';

/**
 * 历史分页的页大小。
 *
 * 取一页而不是取一条：索引接口的成本与条数无关（实测 limit=1 与 limit=40 同为
 * 53ms、响应不足 13KB），而每多取一条就少一次点击、少一次往返。
 *
 * 整页到手后只有最新的一条进回放队列，更早的按索引直接立成折叠行；正文留给
 * 点开某一条时再取。
 */
const HISTORY_PAGE_SIZE = 20;

/** 一次独立回放：展开某一条折叠轮。 */
interface PagingSession {
  /** 临时投影。 */
  state: ConversationState;
  /** 这一路还要回放哪几条。 */
  queue: string[];
  /** 这一次回放自己的订阅标识集合。 */
  ids: Set<string>;
  /** 展开的目标：回放排空后把正文换回这一轮。 */
  replaceTurnId?: string;
}

/** 历史索引里的一条：用户气泡与附件。 */
export interface HistorySeed {
  text: string;
  attachments: AttachmentRef[];
}

/** 历史索引页里的一条；只取这里用得上的字段。 */
export interface HistoryPageEntry {
  interactionId: string;
  content?: string;
  attachments?: readonly string[];
  receivedAtMs: number;
}

export interface LanePagingOptions {
  /** 当前车道标识；会话目标下为空。 */
  laneId: string | null;
  /** 当前工作区标识。 */
  workspaceId: string | null;
  /** 主投影的派发入口。 */
  dispatch: (action: ConversationAction) => void;
  /** 主控制器持有的回放标记与代次：翻页沿用同一套作废语义。 */
  replayingRef: React.RefObject<boolean>;
  generationRef: React.RefObject<number>;
  /** 起一次回放（主控制器的 playNext）。 */
  playNextRef: React.RefObject<
    (workspace: string, generation: number, leaving?: string | null) => void
  >;
  /** 取历史失败时的提示入口。 */
  onError: (message: string) => void;
  /** 车道当前模型的权威事实（网关只读最新一轮记录）；翻到没有结论时给 null。 */
  onEffectiveModel?: (model: LaneEffectiveModel | null) => void;
  /** 首页带回来的最近一次记忆反思提示；没有记忆动作时回 null。 */
  onReflectionNotice?: (notice: string | null) => void;
}

export interface LanePaging {
  /** 取某一条的种子。 */
  seedOf: (interactionId: string) => HistorySeed | undefined;
  /**
   * 最近一页历史带下来的会话累计用量基线；还没取回时为 null。
   *
   * 基线覆盖的轮次一并给出：界面据此只叠加基线之后发生的轮次，不把同一轮算两次。
   */
  sessionBaseline: SessionTokenBaseline | null;
  /** 取最近一页历史：重置种子表与累计基线、记住游标，返回最新一条要回放的标识。 */
  loadNewestPage: (laneId: string) => Promise<string[]>;
  /** 按当前模式派发动作：独立回放时进临时投影，其余进主投影。 */
  applyAction: (action: ConversationAction) => void;
  /** 取下一条要回放的标识：展开时取这一路的队列，否则取主队列。 */
  take: (mainQueue: string[]) => string | undefined;
  /** 独立回放排空后的收口；返回 true 表示这一次排空由这里处理完了。 */
  finish: () => boolean;
  /** 按订阅标识分流一批帧：属于展开那一路的进临时投影，其余进主投影。 */
  routeFrames: (batch: ConversationStreamEvent[], nowMs: number) => void;
  /** 加载更早的一页 Interaction：按索引立成折叠行，正文点开再取。 */
  loadOlder: () => Promise<void>;
  /** 展开一条被折叠的 Interaction：只读尾部一窗。 */
  unfoldTurn: (interactionId: string) => void;
  /** 正在展开的那一条；没有时为空。 */
  unfoldingId: string | null;
  /** 丢弃在途的展开或补窗；目标切换时调用，否则守卫会让新会话点不动。 */
  abort: () => void;
  hasOlder: boolean;
  loadingOlder: boolean;
  /** 前置计数：正文区据此补偿滚动位置。 */
  prependToken: number;
}

/** 组装分页与展开能力。 */
export function useLanePaging(options: LanePagingOptions): LanePaging {
  const { laneId, workspaceId, dispatch, replayingRef, generationRef, playNextRef, onError } =
    options;
  /** 更早一页的游标；为空表示已经翻到车道开头。 */
  const olderCursorRef = useRef<{ receivedAtMs: number; interactionId: string } | null>(null);
  /** 进行中的独立回放。 */
  const pagingRef = useRef<PagingSession | null>(null);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [hasOlder, setHasOlder] = useState(false);
  const [prependToken, setPrependToken] = useState(0);
  /**
   * 正在展开的那一条。
   *
   * 折叠行点开之后要先回放一窗帧，这段时间里它得有个可见的"在动"。只放一个标识：
   * 它变化时该重渲染的只有那一条折叠行，正文各轮的 memo 不该被牵连。
   */
  const [unfoldingId, setUnfoldingId] = useState<string | null>(null);
  /** 历史索引的种子表。 */
  const seedsRef = useRef(new Map<string, HistorySeed>());
  /** 会话累计用量的基线；换车道时整份换掉。 */
  const [sessionBaseline, setSessionBaseline] = useState<SessionTokenBaseline | null>(null);

  const seedHistoryEntries = useCallback((page: readonly HistoryPageEntry[]) => {
    for (const item of page) {
      seedsRef.current.set(item.interactionId, {
        text: item.content ?? '',
        attachments: attachmentsFromPaths([...(item.attachments ?? [])]),
      });
    }
  }, []);

  const seedOf = useCallback(
    (interactionId: string) => seedsRef.current.get(interactionId),
    [],
  );

  /** 取最近一页历史的游标：更早的由「加载更早」按需取。 */
  const rememberFirstPage = useCallback(
    (page: readonly { interactionId: string; receivedAtMs: number }[]) => {
      const oldest = page[0];
      olderCursorRef.current =
        oldest === undefined
          ? null
          : { receivedAtMs: oldest.receivedAtMs, interactionId: oldest.interactionId };
      // 取满一页说明前面可能还有；短页即已经到底。
      setHasOlder(page.length === HISTORY_PAGE_SIZE);
    },
    [],
  );

  /**
   * 取最近一页历史：重置种子表、记住游标，返回这一页要回放的标识。
   *
   * 首屏与重连共用这一处。分页口径只能有一份：页大小、游标起点、种子表归属与
   * 「还有没有更早」都在这里定，调用方不再各自解释「取多少条、从哪往前翻」。
   */
  const loadNewestPage = useCallback(
    async (laneId: string): Promise<string[]> => {
      // 先丢旧基线：累计读数不能先按上一条会话的数画一帧，再跳到新会话。
      setSessionBaseline(null);
      const page = await ipc.conversationHistory(laneId, HISTORY_PAGE_SIZE);
      // 权威模型随首页一起回来：只查最新一轮，不遍历历史轮次。
      options.onEffectiveModel?.(page.effectiveModel ?? null);
      // 记忆提示同一时机取回：它也是「这条会话现在的最新事实」，不能随翻页回退。
      options.onReflectionNotice?.(page.reflectionNotice ?? null);
      seedsRef.current = new Map();
      seedHistoryEntries(page.interactions);
      rememberFirstPage(page.interactions);
      if (page.sessionUsage !== null) {
        setSessionBaseline({
          usage: page.sessionUsage,
          elapsedMs: page.sessionElapsedMs ?? 0,
          turnIds: new Set(page.interactions.map((item) => item.interactionId)),
        });
      }
      /*
       * 只把最新一条交给回放：更早的整页先按索引立成折叠行。
       *
       * 打开长会话不该等几十次串行回放：实测一条 40 轮的车道共 12.4 万帧、96.7MB，
       * 逐条走完要 124 次帧页；而这一页索引只要一次往返。
       */
      const newest = page.interactions[page.interactions.length - 1];
      if (newest === undefined) return [];
      const skeletons = page.interactions.slice(0, -1).map(historySkeletonTurn);
      if (skeletons.length > 0) dispatch({ type: 'prepend-turns', turns: skeletons });
      return [newest.interactionId];
    },
    [dispatch, rememberFirstPage, seedHistoryEntries],
  );

  const applyAction = useCallback(
    (action: ConversationAction) => {
      const paging = pagingRef.current;
      if (paging === null) {
        dispatch(action);
        return;
      }
      paging.state = conversationReducer(paging.state, action);
    },
    [dispatch],
  );

  const take = useCallback((mainQueue: string[]) => {
    const paging = pagingRef.current;
    return paging === null ? mainQueue.shift() : paging.queue.shift();
  }, []);

  /**
   * 丢弃在途的独立回放。
   *
   * 展开与补窗都会把一份临时投影挂在这里，而发起动作前有一道守卫：`pagingRef`
   * 非空就拒绝新的展开与翻页。目标切换时若不把它清掉，新会话的骨架点不开、历史
   * 也翻不动，且用户没有任何办法恢复，只能重启。
   */
  const abort = useCallback(() => {
    pagingRef.current = null;
    setUnfoldingId(null);
    setLoadingOlder(false);
  }, []);

  const finish = useCallback(() => {
    const paging = pagingRef.current;
    if (paging === null) return false;
    pagingRef.current = null;
    setUnfoldingId(null);
    const turnId = paging.replaceTurnId;
    // 独立投影只在展开时建立；没有目标标识就没有要落地的轮次。
    if (turnId === undefined) return true;
    const restored = paging.state.turns.find((turn) => turn.id === turnId);
    dispatch({ type: 'replace-turn-cells', turnId, cells: restored?.cells ?? [] });
    return true;
  }, [dispatch]);

  const routeFrames = useCallback(
    (batch: ConversationStreamEvent[], nowMs: number) => {
      const paging = pagingRef.current;
      if (paging === null) {
        dispatch({ type: 'frames', frames: batch, nowMs });
        return;
      }
      /*
       * 展开期间两路帧同时在跑：属于这一路订阅的进临时投影，其余（实时会话）照旧进
       * 主投影。按订阅标识分流，不靠到达顺序猜。
       */
      const page: ConversationStreamEvent[] = [];
      const live: ConversationStreamEvent[] = [];
      for (const event of batch) {
        (paging.ids.has(event.subscribeId) ? page : live).push(event);
      }
      if (live.length > 0) dispatch({ type: 'frames', frames: live, nowMs });
      if (page.length > 0) paging.state = applyFrames(paging.state, page, nowMs);
    },
    [dispatch],
  );

  /**
   * 加载更早的一页 Interaction。
   *
   * 整页按索引立成折叠行，翻页时一条都不回放：一页二十条只有几 KB，而逐条回放
   * 要按帧页把这段历史整个读一遍。正文留给点开某一条时再取（见 `unfoldTurn`）。
   */
  const loadOlder = useCallback(async () => {
    const cursor = olderCursorRef.current;
    if (cursor === null || laneId === null) return;
    if (pagingRef.current !== null) return;
    const generation = generationRef.current;
    setLoadingOlder(true);
    try {
      const page = await ipc.conversationHistory(laneId, HISTORY_PAGE_SIZE, cursor);
      if (generationRef.current !== generation) return;
      const entries = page.interactions;
      if (entries.length === 0) {
        olderCursorRef.current = null;
        setHasOlder(false);
        return;
      }
      seedHistoryEntries(entries);
      rememberFirstPage(entries);
      dispatch({ type: 'prepend-turns', turns: entries.map(historySkeletonTurn) });
      // 前置一页会让内容整体下移：正文区按同一高度差补偿滚动位置。
      setPrependToken((value) => value + 1);
    } catch (error: unknown) {
      onError(error instanceof Error ? error.message : '加载更早的会话失败');
    } finally {
      setLoadingOlder(false);
    }
  }, [dispatch, generationRef, laneId, onError, rememberFirstPage, seedHistoryEntries]);

  /**
   * 展开一条被折叠的 Interaction。
   *
   * 折叠时正文已经释放，因此这里重新订阅那一条、在独立投影里回放**尾部一窗**，再把
   * 单元整段换回去。回放落地之前该轮一直显示折叠行，不会先空一屏再长出内容。
   *
   * 只读一窗：一条几万帧的轮次整条重放要几十次串行往返，而读者打开它要的是「最近
   * 发生了什么」；更早的部分不再往回补，只由时间线上那句「更早的 N 条内容已折叠」说明。
   */
  const unfoldTurn = useCallback(
    (interactionId: string) => {
      if (pagingRef.current !== null || workspaceId === null) return;
      if (interactionId.trim().length === 0) return;
      pagingRef.current = {
        state: INITIAL_CONVERSATION,
        queue: [interactionId],
        ids: new Set([`interaction:${interactionId}`]),
        replaceTurnId: interactionId,
      };
      setUnfoldingId(interactionId);
      replayingRef.current = true;
      playNextRef.current(workspaceId, generationRef.current);
    },
    [generationRef, playNextRef, replayingRef, workspaceId],
  );

  return useMemo(
    () => ({
      loadNewestPage,
      seedOf,
      applyAction,
      take,
      finish,
      routeFrames,
      loadOlder,
      unfoldTurn,
      unfoldingId,
      abort,
      hasOlder,
      loadingOlder,
      prependToken,
      sessionBaseline,
    }),
    [
      applyAction,
      finish,
      hasOlder,
      loadNewestPage,
      loadOlder,
      loadingOlder,
      prependToken,
      routeFrames,
      seedOf,
      sessionBaseline,
      take,
      unfoldTurn,
      unfoldingId,
      abort,
    ],
  );
}
