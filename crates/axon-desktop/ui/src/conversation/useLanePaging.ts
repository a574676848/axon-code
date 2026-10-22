/*
 * 车道历史的分页与展开。
 *
 * 两件事共用一个机制：**先在独立投影里回放，再把算好的轮次整段接回主投影**。
 *
 * 不能让它们直接在主投影里回放：帧的归属规则是「最后一条尚未收口的轮次」，而在主投影
 * 里回放历史会把帧记到最新的轮次上。因此这里自己持有一份临时投影与它自己的订阅标识
 * 集合，回放期间两路帧按订阅标识分流，回放排空后前置一页，或把某一轮的正文换回来。
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
  type AttachmentRef,
  type ConversationAction,
  type ConversationState,
} from './transcript';
import type { ConversationStreamEvent } from './wire';

/** 历史分页的页大小：一次一条 Interaction，向前翻就是逐条往前取。 */
const HISTORY_PAGE_SIZE = 1;

/** 一次独立回放。 */
interface PagingSession {
  /** 临时投影。 */
  state: ConversationState;
  /** 这一页还要回放哪几条。 */
  queue: string[];
  /** 这一次回放自己的订阅标识集合。 */
  ids: Set<string>;
  /** 给出时表示这次回放是为了展开某一轮，而不是向前翻页。 */
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
}

export interface LanePaging {
  /** 取某一条的种子。 */
  seedOf: (interactionId: string) => HistorySeed | undefined;
  /** 取最近一页历史：重置种子表、记住游标，返回这一页要回放的标识。 */
  loadNewestPage: (laneId: string) => Promise<string[]>;
  /** 按当前模式派发动作：独立回放时进临时投影，其余进主投影。 */
  applyAction: (action: ConversationAction) => void;
  /** 取下一条要回放的标识：翻页时取这一页的队列，否则取主队列。 */
  take: (mainQueue: string[]) => string | undefined;
  /** 独立回放排空后的收口；返回 true 表示这一次排空由这里处理完了。 */
  finish: () => boolean;
  /** 按订阅标识分流一批帧：属于这一页的进临时投影，其余进主投影。 */
  routeFrames: (batch: ConversationStreamEvent[], nowMs: number) => void;
  /** 加载更早的一条 Interaction。 */
  loadOlder: () => Promise<void>;
  /** 展开一条被折叠的 Interaction。 */
  unfoldTurn: (interactionId: string) => void;
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
  /** 历史索引的种子表。 */
  const seedsRef = useRef(new Map<string, HistorySeed>());

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
      const page = await ipc.conversationHistory(laneId, HISTORY_PAGE_SIZE);
      seedsRef.current = new Map();
      seedHistoryEntries(page);
      rememberFirstPage(page);
      return page.map((item) => item.interactionId);
    },
    [rememberFirstPage, seedHistoryEntries],
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

  const finish = useCallback(() => {
    const paging = pagingRef.current;
    if (paging === null) return false;
    pagingRef.current = null;
    if (paging.replaceTurnId === undefined) {
      // 更早那一页回放完毕：整段前置，并让正文区补偿滚动位置。
      dispatch({ type: 'prepend-turns', turns: paging.state.turns });
      setPrependToken((value) => value + 1);
      return true;
    }
    // 展开一条折叠的历史：把重新回放算出来的单元整段换回那一轮。
    const restored = paging.state.turns.find((turn) => turn.id === paging.replaceTurnId);
    dispatch({
      type: 'replace-turn-cells',
      turnId: paging.replaceTurnId,
      cells: restored?.cells ?? [],
    });
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
       * 翻页期间两路帧同时在跑：属于这一页订阅的进临时投影，其余（实时会话）照旧进
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
   * 加载更早的一条 Interaction。
   *
   * 逐条向前翻：取到的那一条先在独立投影里回放完，再整段前置到时间线头部，正文区
   * 据此补偿滚动位置。取到空页即到底，按钮随之消失。
   */
  const loadOlder = useCallback(async () => {
    const cursor = olderCursorRef.current;
    if (cursor === null || laneId === null || workspaceId === null) return;
    if (pagingRef.current !== null) return;
    const generation = generationRef.current;
    setLoadingOlder(true);
    try {
      const page = await ipc.conversationHistory(laneId, HISTORY_PAGE_SIZE, cursor);
      if (generationRef.current !== generation) return;
      if (page.length === 0) {
        olderCursorRef.current = null;
        setHasOlder(false);
        return;
      }
      seedHistoryEntries(page);
      rememberFirstPage(page);
      pagingRef.current = {
        state: INITIAL_CONVERSATION,
        queue: page.map((item) => item.interactionId),
        ids: new Set(page.map((item) => `interaction:${item.interactionId}`)),
      };
      replayingRef.current = true;
      playNextRef.current(workspaceId, generation);
    } catch (error: unknown) {
      onError(error instanceof Error ? error.message : '加载更早的会话失败');
    } finally {
      setLoadingOlder(false);
    }
  }, [
    generationRef,
    laneId,
    onError,
    playNextRef,
    rememberFirstPage,
    replayingRef,
    seedHistoryEntries,
    workspaceId,
  ]);

  /**
   * 展开一条被折叠的 Interaction。
   *
   * 折叠时正文已经释放，因此这里重新订阅那一条、在独立投影里回放完，再把单元整段
   * 换回去。回放落地之前该轮一直显示折叠行，不会先空一屏再长出内容。
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
      hasOlder,
      loadingOlder,
      prependToken,
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
      take,
      unfoldTurn,
    ],
  );
}
