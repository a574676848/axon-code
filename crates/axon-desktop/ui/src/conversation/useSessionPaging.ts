/*
 * 车道历史的打开、按轮翻页与展开。
 *
 * 轮次大纲加事件窗口：索引给出「每一轮问了什么、算了多久、最后答了什么」，客户端只把
 * **已加载的一轮**渲染成完整正文。三档粒度：
 *
 * - **打开**：一次索引页（默认 20 条）立刻立起三形态（用户气泡 + 过程抬头 + 最终回复），
 *   随后只把**最新一轮**读整轮换成展开形态。首屏不等历史帧，打开成本不随轮数增长。
 * - **加载更早**：一次一轮（`OLDER_STEP`），取回来立起三形态。滚到顶部才发一次，
 *   不整批读正文。
 * - **展开**：点开过程抬头才读整轮；收起即释放（`collapse-turn` 丢掉过程单元）。
 *
 * 帧一律喂进与实时同一个归约器；索引提供帧里没有的那几项：附件、逐轮来源，以及每轮的
 * 指标读数（用时、token、首字、速率）。
 */

import { useCallback, useMemo, useRef, useState } from 'react';
import * as ipc from '../ipc';
import type { TurnMetricsReading } from '../types';
import {
  historySource,
  insertControlAfterInput,
  interactionStillRunning,
  sessionJobRows,
  projectFrames,
  readingFactsOf,
  turnSummaryFacts,
  type HistoryFacts,
} from './historyProjection';
import { readInteractionFrames, readInteractionFramesTailFirst } from './interactionReplay';
import { newestReceivedAtMs, newestExpandable, isAuditTurn, turnsNewerThanFloor } from './pagingBounds';
import { provisionalFrames, withOpeningInput, type HistoryEntry } from './pagingSkeleton';
import {
  INITIAL_CONVERSATION,
  applyFrames,
  attachmentsFromPaths,
  conversationReducer,
  outlineControl,
  planProjectionOf,
  summaryFromIndexFacts,
  type ConversationAction,
  type JobRow,
  type TurnState,
} from './transcript';
import { applyTurnSummary, seedTurnReadings, settleTurn, type TurnSummaryFacts } from './transcript/turns';
import type { SessionTokenBaseline } from './sessionUsage';
import { sessionFactsOf, type SessionFacts } from './sessionFacts';
import type { ConversationStreamEvent } from './wire';
import type { SessionEffectiveModel } from './viewConfig';

/** 首屏索引页大小：一次拿到最近一页的轮次清单，不参与正文回放。 */
const HISTORY_PAGE_SIZE = 20;
/** 「加载更早」每次取的轮数：按轮翻页，不是把更早的一页整批取回来。 */
const OLDER_STEP = 1;

export type { HistoryFacts } from './historyProjection';
export type { HistoryEntry } from './pagingSkeleton';

export interface SessionPagingOptions {
  /** 当前车道标识；会话目标下为空。 */
  sessionId: string | null;
  /** 当前工作区标识。 */
  workspaceId: string | null;
  /** 主投影的派发入口。 */
  dispatch: (action: ConversationAction) => void;
  /** 当前代次；目标切换后旧回放据此作废。 */
  generationRef: React.RefObject<number>;
  /** 取历史失败时的提示入口。 */
  onError: (message: string) => void;
  /** 主投影当前持有的轮次标识：增量接管据此只接还不认识的那几条。 */
  knownTurnIds: () => readonly string[];
  /** 仍有轮次在跑时，从 durable 尾订阅它的增量。 */
  subscribeTail: (interactionId: string, cursor: string | null) => Promise<void>;
  /** 车道当前模型的权威事实（网关只读最新一轮记录）；读不到时给 null。 */
  onEffectiveModel?: (model: SessionEffectiveModel | null) => void;
  /** 首页带回来的最近一次记忆反思提示；没有记忆动作时回 null。 */
  onReflectionNotice?: (notice: string | null) => void;
  /** 首页带回来的会话能力事实：可写性、可中断与可答问都读它。 */
  onSessionFacts?: (facts: SessionFacts | null) => void;

}

export interface SessionPaging {
  /** 最近一页索引带下来的会话累计用量基线；还没取回时为 null。 */
  sessionBaseline: SessionTokenBaseline | null;
  /** 打开一条车道：索引立起三形态，再把最新一轮换成展开形态。 */
  openSession: (sessionId: string) => Promise<void>;
  /** 加载更早的一轮：一次一条。 */
  loadOlder: () => Promise<void>;
  /** 把外部发起的新一轮接进投影：只接还不认识的那几条。 */
  appendSession: () => Promise<void>;
  /** 展开一条被折叠的 Interaction：读整轮帧后替换该轮单元。 */
  unfoldTurn: (interactionId: string) => void;
  /** 正在展开的那一条；没有时为空。 */
  unfoldingId: string | null;
  hasOlder: boolean;
  loadingOlder: boolean;
  /** 索引或补帧还在途：过渡窗口的兜底计时据此让位。 */
  pageLoading: boolean;
  /** 前置计数：正文区据此补偿滚动位置。 */
  prependToken: number;
}

/** 组装分页与展开能力。 */
export function useSessionPaging(options: SessionPagingOptions): SessionPaging {
  const {
    sessionId,
    workspaceId,
    dispatch,
    generationRef,
    onError,
    knownTurnIds,
    subscribeTail,
    onEffectiveModel,
    onReflectionNotice,
    onSessionFacts,
  } = options;
  /** 更早一轮的游标；为空表示已经翻到车道开头。 */
  const olderCursorRef = useRef<{ receivedAtMs: number; interactionId: string } | null>(null);
  /**
   * 已加载轮次里最新的接收时刻。
   *
   * 增量接管按「本视图还不认识」去重，但补回的老轮同样不认识：向前翻过页之后，最新一页
   * 窗口里会同时含已被替换成历史的老轮，把它们当新轮追加会让旧对话出现在对话末尾。
   * 因此再按时间设一道下界：只有不早于已加载最新时刻的轮次才算新轮。
   */
  const newestReceivedAtRef = useRef<number | null>(null);
  /** 历史索引的补充事实表（附件与来源）。 */
  const factsRef = useRef(new Map<string, HistoryFacts>());
  /**
   * 索引给出的逐轮读数。
   *
   * 打开与翻页时随索引一起入库，展开某轮时按标识取回：`turn_metrics` 是这一轮读数的
   * 权威事实，即使展开后只回放出尾窗帧，分割线也照样完整。
   */
  const readingsRef = useRef(new Map<string, TurnMetricsReading>());
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [pageLoading, setPageLoading] = useState(false);
  const [hasOlder, setHasOlder] = useState(false);
  const [prependToken, setPrependToken] = useState(0);
  const [unfoldingId, setUnfoldingId] = useState<string | null>(null);
  /** 会话累计用量的基线；换车道时整份换掉。 */
  const [sessionBaseline, setSessionBaseline] = useState<SessionTokenBaseline | null>(null);
  /** 展开的防重入：同一条正在读帧时再点不重复发起。 */
  const unfoldingRef = useRef<string | null>(null);

  const seedHistoryFacts = useCallback((page: readonly HistoryEntry[]) => {
    for (const item of page) {
      factsRef.current.set(item.interactionId, {
        attachments: attachmentsFromPaths([...(item.attachments ?? [])]),
        channel: historySource(item),
      });
      const metrics = item.metrics ?? null;
      if (metrics !== null) {
        readingsRef.current.set(item.interactionId, metrics);
      }
    }
  }, []);

  /**
   * 一轮的分割线读数：索引给出的 `turn_metrics` 优先，缺失时退回帧算出来的那份。
   */
  const readingFactsFor = useCallback(
    (interactionId: string, fallbackElapsedMs: number | null, turn: TurnState): TurnSummaryFacts =>
      turnSummaryFacts(readingsRef.current.get(interactionId), fallbackElapsedMs, turn),
    [],
  );

  /** 索引读数：把 `turn_metrics` 换成归约器认得的形状，供指标条在帧回来之前成立。 */
  const indexReadings = useCallback(
    (interactionId: string) => readingFactsOf(readingsRef.current.get(interactionId)),
    [],
  );

  /** 记住下一次前翻的游标，并判定车道是否还有更早的轮次。 */
  const rememberOldest = useCallback((entry: HistoryEntry | undefined, more: boolean) => {
    if (entry !== undefined) {
      olderCursorRef.current = {
        receivedAtMs: entry.receivedAtMs,
        interactionId: entry.interactionId,
      };
    }
    setHasOlder(more);
  }, []);

  const fetchNewestPage = useCallback(
    async (laneId: string) => {
      const page = await ipc.conversationHistory(laneId, HISTORY_PAGE_SIZE);
      // 权威模型随首页一起回来：只查最新一轮，不遍历历史轮次。
      onEffectiveModel?.(page.effectiveModel ?? null);
      // 会话能力同一时机取回：可写性与能力位是「此刻这条会话能做什么」的唯一判据。
      onSessionFacts?.(sessionFactsOf(page.session));
      // 记忆提示同一时机取回：它也是「这条会话现在的最新事实」，不能随翻页回退。
      onReflectionNotice?.(page.reflectionNotice ?? null);
      /*
       * 计划快照只交给调用方，不在这里派发：它必须在换会话的 `reset` 之后落地，
       * 否则刚读回来的快照会被紧接着的重置擦掉（自己抹一遍自己的新鲜事实）。
       */
      const snapshot = page.sessionPlan ?? null;
      const plan = snapshot === null ? null : planProjectionOf(snapshot.title, snapshot.steps);

      if (page.sessionUsage !== null) {
        setSessionBaseline({
          usage: page.sessionUsage,
          elapsedMs: page.sessionElapsedMs ?? 0,
          turnIds: new Set(page.interactions.map((item) => item.interactionId)),
          // 取页那一刻仍在跑的那几条：账本还没落账，整条由本地增量承担。
          openTurnIds: new Set(
            page.interactions
              .filter((item) => interactionStillRunning(item.status))
              .map((item) => item.interactionId),
          ),
        });
      }
      // 会话级后台作业清单随索引一起回来：作业属于整条会话，不能只靠本轮回放。
      const jobs = sessionJobRows(page.jobs);
      if (jobs.length > 0) dispatch({ type: 'seed-jobs', jobs });
      return { entries: page.interactions as HistoryEntry[], plan };
    },
    [dispatch, onEffectiveModel, onReflectionNotice, onSessionFacts],
  );

  /**
   * 把一轮投影出来：有帧就纯按帧投影，没有帧才用索引骨架。
   *
   * 两条路径不能叠：索引骨架里那条合成的用户消息会与帧里的真实用户消息各成一格，
   * 而骨架自带的收口帧又会先把这一轮判成结束，让随后的回放帧另起一轮（实测表现为
   * 「打开会话后最新一轮的过程行整段不见、上下文读数也空着」）。因此这里二选一。
   *
   * `truncated` 是回放自己交回来的事实：这一轮中间一段没读到，正文要如实标一句。
   */
  const projectTurn = useCallback(
    (
      entry: HistoryEntry,
      frames: readonly ConversationStreamEvent[] | null,
      truncated = false,
      /*
       * 这一轮在服务端是否已经收口。
       *
       * 有帧时由 `InteractionFrames.settled` 给（帧里有没有终态帧），没有帧的骨架路径按
       * 索引里的 lifecycle 判。两条路都不替服务端下结论：还在跑的一轮必须留在
       * `streaming`，否则 `working` 当场变假，工作状态行与侧栏转圈一起消失。
       */
      settled = !interactionStillRunning(entry.status),
    ): { turn: TurnState; jobs: JobRow[] } => {
      if (frames !== null && frames.length > 0) {
        return projectFrames(
          entry.interactionId,
          frames,
          factsRef.current.get(entry.interactionId),
          readingsRef.current.get(entry.interactionId),
          entry.elapsedMs ?? null,
          { truncated, settled },
        );
      }
      let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn',
        turnId: entry.interactionId,
        nowMs: entry.receivedAtMs,
      });
      const facts = factsRef.current.get(entry.interactionId);
      if (facts !== undefined) {
        state = conversationReducer(state, {
          type: 'attach-history',
          interactionId: entry.interactionId,
          attachments: facts.attachments,
          channel: facts.channel,
        });
      }
      state = applyFrames(state, provisionalFrames(entry), entry.receivedAtMs);
      const turn = state.turns[0];
      if (turn === undefined) {
        return { turn: state.turns[0], jobs: state.jobs };
      }
      const closed =
        settled && (turn.status === 'streaming' || turn.status === 'pending')
          ? settleTurn(turn, 'completed', entry.elapsedMs ?? null)
          : turn;
      /*
       * 索引读数先落到这一轮上：首字、速率与用量在帧回来之前就要在指标条上成立。
       * 窗口占用不在索引里，因此这一轮只有 token 分桶，上下文段仍等收尾负载。
       */
      const seeded = seedTurnReadings(closed, indexReadings(entry.interactionId));
      /*
       * 只读索引的那一轮：给一根折叠行当入口。
       *
       * 过程要等点开才读整轮，但读者需要一个「这一轮可以展开」的落点；标题只写中性的
       * 「过程」，点开之后换成帧算出来的真实动作摘要（没有过程时这一行会消失）。
       */
      const elapsed = readingFactsFor(entry.interactionId, entry.elapsedMs ?? null, seeded).durationMs;
      /*
       * 没有过程证据就不立折叠行。
       *
       * 纯问答轮点开也没有过程可看，给它立一根「过程」抬头只是把一次问答伪装成跑过
       * 东西的回合。判据取索引里的事实：这一轮有工具名、有工具调用次数或有推理输出，
       * 三者都没有才认定它没有过程。
       */
      const reading = readingsRef.current.get(entry.interactionId);
      const hasProcess =
        (entry.toolNames?.length ?? 0) > 0 ||
        (reading?.toolCalls ?? 0) > 0 ||
        (reading?.reasoningTokens ?? 0) > 0;
      const withControl = hasProcess
        ? insertControlAfterInput(
            seeded,
            outlineControl(
              entry.interactionId,
              entry.interactionId,
              entry.receivedAtMs,
              elapsed,
              summaryFromIndexFacts(entry.toolNames ?? [], reading?.reasoningTokens),
            ),
          )
        : seeded;
      return {
        turn: applyTurnSummary(
          withControl,
          readingFactsFor(entry.interactionId, entry.elapsedMs ?? null, withControl),
        ),
        jobs: state.jobs,
      };
    },
    [indexReadings, provisionalFrames, readingFactsFor],
  );

  /** 一条轮次的骨架形态（打开时光标还没落到任何历史帧上）。 */
  const withProvisional = useCallback(
    (entries: readonly HistoryEntry[]) =>
      entries.filter((entry) => !isAuditTurn(entry)).map((entry) => projectTurn(entry, null).turn),
    [projectTurn],
  );

  /**
   * 帧读不到时的收口兜底。
   *
   * 骨架已经按「在跑」立起来了，但这一轮此刻没有任何证据（帧没读到，实时尾也没订上）：
   * 继续挂着 `streaming` 会让工作状态行与侧栏转圈永远转下去，比少一段过程更误导。
   * 退回按历史收口的骨架，失败本身仍由调用方如实上报。
   */
  const settleUnread = useCallback(
    (entry: HistoryEntry) => {
      const settled = projectTurn(entry, null, false, true);
      dispatch({ type: 'replace-turn', turn: settled.turn, jobs: settled.jobs });
    },
    [dispatch, projectTurn],
  );

  /**
   * 打开一条车道：索引立起三形态，再升级最新一轮为展开形态。
   *
   * 首屏在索引返回时就交出，不等历史帧；最新一轮的回放随后替换该轮单元。每轮的分割线
   * 由索引里的 `turn_metrics` 直接给出，因此打开成本 = 一次索引 + 一轮整帧，不随轮数增长。
   */
  const openSession = useCallback(
    async (laneId: string): Promise<void> => {
      // 先丢旧基线：累计读数不能先按上一条会话的数画一帧，再跳到新会话。
      setSessionBaseline(null);
      // 能力事实同理：上一条会话的可写性不能沿用，否则切会话会短暂放出错误的写入口。
      onSessionFacts?.(null);
      // 翻页游标同样属于上一条会话：失败路径不会再走到下面的成功分支，残留的旧游标会被
      // 下一次「加载更早」拿去查新会话，取回该时刻之前的一条前置进正文即形成缺轮。
      olderCursorRef.current = null;
      setHasOlder(false);
      const generation = generationRef.current;
      setPageLoading(true);
      try {
        const { entries, plan } = await fetchNewestPage(laneId);
        if (generation !== generationRef.current) return;
        factsRef.current = new Map();
        readingsRef.current = new Map();
        seedHistoryFacts(entries);
        newestReceivedAtRef.current = newestReceivedAtMs(entries);
        rememberOldest(entries[0], entries.length >= HISTORY_PAGE_SIZE);
        dispatch({ type: 'reset' });
        // 快照在重置之后才落地：先 reset 再 seed，首屏就有当前计划，且不会被抹掉。
        if (plan !== null) dispatch({ type: 'plan-seed', plan });
        if (entries.length > 0) {
          dispatch({ type: 'prepend-turns', turns: withProvisional(entries) });
        }
        const newest = newestExpandable(entries) ?? entries[entries.length - 1];
        if (newest === undefined) return;
        /*
         * 最新一轮走「尾窗先行」：一次往返把这一轮的结论与最近的过程先出屏，前段随后在
         * 后台补齐并整轮重投。
         *
         * 这里只等尾窗：过渡面与「正在加载会话」都以它为准。若把补齐也 await 掉，超大
         * 轮次的首屏仍会被后台往返拖住，等于没做这条读法。
         */
        const reading = readInteractionFramesTailFirst(newest.interactionId, generation, {
          generation: () => generationRef.current,
          onTail: () => undefined,
        });
        const tail = await reading.tail;
        let tailShown = false;
        if (tail !== null && generation === generationRef.current) {
          tailShown = true;
          const projected = projectTurn(
            newest,
            withOpeningInput(newest, tail),
            tail.truncated,
            tail.settled,
          );
          dispatch({ type: 'replace-turn', turn: projected.turn, jobs: projected.jobs });
        }
        void reading.full
          .then((full) => {
            if (full === null || generation !== generationRef.current) return;
            const projected = projectTurn(
              newest,
              withOpeningInput(newest, full),
              full.truncated,
              full.settled,
            );
            dispatch({ type: 'replace-turn', turn: projected.turn, jobs: projected.jobs });
            if (!full.settled && workspaceId !== null) {
              void subscribeTail(newest.interactionId, full.cursor);
            }
          })
          .catch((error: unknown) => {
            // 首屏已经有意料中的内容时不退回骨架：退回只会把正文抹成空壳。
            if (!tailShown) settleUnread(newest);
            onError(error instanceof Error ? error.message : '读取最新一轮失败');
          });
      } catch (error: unknown) {
        /*
         * 打开失败必须把投影清空：否则上一条会话的正文会以新会话的身份继续渲染，而唯一的
         * 提示只是输入坞里一条到点自动退场的回执。这里只做复位与上报，重试入口由调用方给。
         */
        if (generation === generationRef.current) {
          factsRef.current = new Map();
          readingsRef.current = new Map();
          newestReceivedAtRef.current = null;
          dispatch({ type: 'reset' });
        }
        throw error;
      } finally {
        setPageLoading(false);
      }
    },
    [
      dispatch,
      fetchNewestPage,
      generationRef,
      onError,
      projectTurn,
      rememberOldest,
      seedHistoryFacts,
      settleUnread,
      subscribeTail,
      withOpeningInput,
      withProvisional,
      onSessionFacts,
      workspaceId,
    ],
  );

  /**
   * 加载更早的轮次：一次一轮。
   *
   * 滚到顶部才发一次，取回来立刻立起三形态（正文区按前置高度差补偿滚动位置）。索引已经
   * 带上这一轮的读数，因此折叠行与分割线当场就有数，不必再为它们多读一帧。车道再长也
   * 不会一次读回一页正文。
   */
  const loadOlder = useCallback(async () => {
    const cursor = olderCursorRef.current;
    if (cursor === null || sessionId === null) return;
    const generation = generationRef.current;
    setLoadingOlder(true);
    try {
      const page = await ipc.conversationHistory(sessionId, OLDER_STEP, cursor);
      if (generation !== generationRef.current) return;
      const entries = page.interactions as HistoryEntry[];
      if (entries.length === 0) {
        olderCursorRef.current = null;
        setHasOlder(false);
        return;
      }
      seedHistoryFacts(entries);
      rememberOldest(entries[0], true);
      dispatch({ type: 'prepend-turns', turns: withProvisional(entries) });
      setPrependToken((value) => value + 1);
    } catch (error: unknown) {
      onError(error instanceof Error ? error.message : '加载更早的会话失败');
    } finally {
      setLoadingOlder(false);
    }
  }, [
    dispatch,
    generationRef,
    onError,
    rememberOldest,
    seedHistoryFacts,
    sessionId,
    withProvisional,
  ]);

  /**
   * 增量接管：只接本视图还不认识的轮次。
   *
   * 新的一轮照最新一轮处理（读整轮、保持展开）。翻页游标不动；分割线的读数由索引给出。
   */
  const appendSession = useCallback(async (): Promise<void> => {
    if (sessionId === null || workspaceId === null) return;
    const generation = generationRef.current;
    try {
      const { entries, plan } = await fetchNewestPage(sessionId);
      if (generation !== generationRef.current) return;
      seedHistoryFacts(entries);
      // 计划坞为空时用这份快照补上；已有计划（回放或实时帧给过）时归约器会忽略它。
      if (plan !== null) dispatch({ type: 'plan-seed', plan });
      const known = new Set(knownTurnIds());
      // 审计轮不进投影，也就不该触发读帧：它们只有一句系统说明，没有 durable 事件，
      // 去读只会换来一条「交互记录不存在」的错误卡。
      const fresh = turnsNewerThanFloor(
        entries.filter((entry) => !isAuditTurn(entry)),
        known,
        (entry) => entry.interactionId,
        newestReceivedAtRef.current,
      );
      const newestSeen = newestReceivedAtMs(entries);
      const floor = newestReceivedAtRef.current;
      if (newestSeen !== null && (floor === null || newestSeen > floor)) {
        newestReceivedAtRef.current = newestSeen;
      }
      if (fresh.length === 0) return;
      dispatch({ type: 'append-turns', turns: withProvisional(fresh) });
      const newest = fresh[fresh.length - 1];
      /*
       * 一条新轮接管：先要尾窗，再在后台补整轮。
       *
       * 这一轮的帧读不到（例如刚被清理）不该把整条会话变成错误：骨架已经立起来，正文缺
       * 过程只是少了一段，下一次轮次头会再补。
       */
      const takeOver = async (entry: HistoryEntry, expand: boolean): Promise<void> => {
        const reading = readInteractionFramesTailFirst(entry.interactionId, generation, {
          generation: () => generationRef.current,
          onTail: () => undefined,
        });
        const tail = await reading.tail;
        let tailShown = false;
        if (tail !== null && generation === generationRef.current) {
          tailShown = true;
          const projected = projectTurn(
            entry,
            withOpeningInput(entry, tail),
            tail.truncated,
            tail.settled,
          );
          dispatch({ type: 'replace-turn', turn: projected.turn, jobs: projected.jobs });
          // 只有最新一条走「整轮展开」，其余新轮拿到尾窗就够；但它们仍在跑时必须各订一条
          // 实时尾，否则会永久停在骨架的 streaming 态。
          if (!expand && !tail.settled) void subscribeTail(entry.interactionId, tail.cursor);
        }
        if (!expand) return;
        void reading.full
          .then((full) => {
            if (full === null || generation !== generationRef.current) return;
            const projected = projectTurn(
              entry,
              withOpeningInput(entry, full),
              full.truncated,
              full.settled,
            );
            dispatch({ type: 'replace-turn', turn: projected.turn, jobs: projected.jobs });
            if (!full.settled) void subscribeTail(entry.interactionId, full.cursor);
          })
          .catch(() => {
            // 读不到帧就把骨架收口：骨架是按「还在跑」立起来的，此刻却没有任何证据支持
            // 这一轮仍在推进（帧没读到，实时尾也订不上）。让转圈永远转下去比少一段过程
            // 更误导；尾窗已经出屏时不退回，正文已经有内容。
            if (generation === generationRef.current && !tailShown) settleUnread(entry);
          });
      };
      /*
       * 每一条新轮都要接管，而不是只给最后一条读帧：前面仍在跑的那几条会永久停在骨架的
       * streaming 态（轮次头再触发也会被「已认识」去重过滤），工作状态行恒转，用户随后的
       * 每条消息都会被当成 steer 排进一个永不收口的轮次。
       */
      const running = fresh.filter((entry) => entry.status !== 'understood');
      const targets = running.includes(newest) ? running : [...running, newest];
      await Promise.all(targets.map((entry) => takeOver(entry, entry === newest)));
    } catch (error: unknown) {
      onError(error instanceof Error ? error.message : '读取新轮次失败');
    }
  }, [
    dispatch,
    fetchNewestPage,
    generationRef,
    knownTurnIds,
    onError,
    projectTurn,
    seedHistoryFacts,
    sessionId,
    settleUnread,
    subscribeTail,
    withOpeningInput,
    withProvisional,
    workspaceId,
  ]);

  /**
   * 展开一条被折叠的 Interaction。
   *
   * 折叠时过程已经撤下，因此这里按同一条读帧口径把整轮重新算一遍，再把单元整段换回去。
   * 回放落地之前该轮一直显示过程控件，不会先空一屏再长出内容。收起由 `collapse-turn`
   * 负责，那一步会把过程单元整段丢掉（数据随之释放）。
   */
  const unfoldTurn = useCallback(
    (interactionId: string) => {
      if (interactionId.trim().length === 0 || unfoldingRef.current !== null) return;
      const generation = generationRef.current;
      unfoldingRef.current = interactionId;
      setUnfoldingId(interactionId);
      void readInteractionFrames(interactionId, generation, {
        generation: () => generationRef.current,
      })
        .then((read) => {
          if (read === null || generation !== generationRef.current) return;
          /*
           * 展开只按帧投影，不再叠索引骨架：骨架那条合成的用户消息会与帧里的真实
           * 用户消息各成一格，正文里就出现两个一样的气泡。
           */
          const projected = projectFrames(
            interactionId,
            read.frames,
            factsRef.current.get(interactionId),
            readingsRef.current.get(interactionId),
            null,
            { truncated: read.truncated, settled: read.settled },
          );
          dispatch({
            type: 'replace-turn',
            turn: projected.turn,
            mode: 'expanded',
            jobs: projected.jobs,
          });
          if (!read.settled && workspaceId !== null) {
            void subscribeTail(interactionId, read.cursor);
          }
        })
        .catch((error: unknown) => {
          onError(error instanceof Error ? error.message : '展开这一轮失败');
        })
        .finally(() => {
          unfoldingRef.current = null;
          setUnfoldingId(null);
        });
    },
    [dispatch, generationRef, onError, projectFrames, subscribeTail, workspaceId],
  );

  return useMemo(
    () => ({
      sessionBaseline,
      openSession,
      loadOlder,
      appendSession,
      unfoldTurn,
      unfoldingId,
      hasOlder,
      loadingOlder,
      pageLoading,
      prependToken,
    }),
    [
      appendSession,
      hasOlder,
      loadOlder,
      loadingOlder,
      openSession,
      pageLoading,
      prependToken,
      sessionBaseline,
      unfoldTurn,
      unfoldingId,
    ],
  );
}

/**
 * 只按帧投影一轮（展开用）。
 *
 * 不叠索引骨架：骨架里那条合成的用户消息与帧里的真实用户消息各成一格，正文里就会
 * 出现两个一样的气泡。附件与来源仍从索引贴进来，帧里没有这两样东西；分割线的读数
 * 以索引里的 `turn_metrics` 定稿，回放只负责把过程正文长回来。
 */
