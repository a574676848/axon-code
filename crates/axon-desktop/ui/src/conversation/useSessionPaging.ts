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
  completedFrame,
  historySource,
  insertControlAfterInput,
  sessionJobRows,
  projectFrames,
  readingFactsOf,
  statusFrame,
  turnSummaryFacts,
  wireFrame,
  type HistoryFacts,
} from './historyProjection';
import { readInteractionFrames } from './interactionReplay';
import {
  INITIAL_CONVERSATION,
  applyFrames,
  attachmentsFromPaths,
  conversationReducer,
  outlineControl,
  summaryFromToolNames,
  type ConversationAction,
  type JobRow,
  type TurnState,
} from './transcript';
import { applyTurnSummary, seedTurnReadings, settleTurn, type TurnSummaryFacts } from './transcript/turns';
import type { SessionTokenBaseline } from './sessionUsage';
import type { ConversationStreamEvent } from './wire';
import type { SessionEffectiveModel } from './viewConfig';

/** 首屏索引页大小：一次拿到最近一页的轮次清单，不参与正文回放。 */
const HISTORY_PAGE_SIZE = 20;
/** 「加载更早」每次取的轮数：按轮翻页，不是把更早的一页整批取回来。 */
const OLDER_STEP = 1;

export type { HistoryFacts } from './historyProjection';

/** 索引页里一条 Interaction 的界面相关字段。 */
interface HistoryEntry {
  interactionId: string;
  receivedAtMs: number;
  status: string;
  content?: string;
  answerText?: string | null;
  /** 老网关才有：没有指标事件时的墙钟用时。 */
  elapsedMs?: number | null;
  /** 这一轮的读数；事实源是该轮自己的 `turn_metrics` 事件。 */
  metrics?: TurnMetricsReading | null;
  /** 这一轮用过的工具名；索引轮据此立起动作摘要。 */
  toolNames?: readonly string[];
  attachments?: readonly string[];
  channel?: string;
  client?: string;
}

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
  } = options;
  /** 更早一轮的游标；为空表示已经翻到车道开头。 */
  const olderCursorRef = useRef<{ receivedAtMs: number; interactionId: string } | null>(null);
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
      // 记忆提示同一时机取回：它也是「这条会话现在的最新事实」，不能随翻页回退。
      onReflectionNotice?.(page.reflectionNotice ?? null);
      if (page.sessionUsage !== null) {
        setSessionBaseline({
          usage: page.sessionUsage,
          elapsedMs: page.sessionElapsedMs ?? 0,
          turnIds: new Set(page.interactions.map((item) => item.interactionId)),
          // 取页那一刻仍在跑的那几条：账本还没落账，整条由本地增量承担。
          openTurnIds: new Set(
            page.interactions
              .filter((item) => item.status === 'received' || item.status === 'processing')
              .map((item) => item.interactionId),
          ),
        });
      }
      // 会话级后台作业清单随索引一起回来：作业属于整条会话，不能只靠本轮回放。
      const jobs = sessionJobRows(page.jobs);
      if (jobs.length > 0) dispatch({ type: 'seed-jobs', jobs });
      return page.interactions as HistoryEntry[];
    },
    [dispatch, onEffectiveModel, onReflectionNotice],
  );

  /**
   * 索引给出的三形态骨架帧：用户说了什么、最后答了什么。
   *
   * 用户单元用本地标识落位，真正的 durable 用户帧到达时会「认领」它并换成服务端标识，
   * 因此三形态与补帧后的正文之间不会出现两条气泡。
   */
  const provisionalFrames = useCallback((entry: HistoryEntry): ConversationStreamEvent[] => {
    const frames: ConversationStreamEvent[] = [
      wireFrame(entry.interactionId, entry.receivedAtMs, 'userMessage', {
        partId: `local-user:${entry.interactionId}`,
        messageId: `${entry.interactionId}:user`,
        content: entry.content ?? '',
      }),
    ];
    const answer = (entry.answerText ?? '').trim();
    if (entry.status === 'interrupted') {
      frames.push(statusFrame(entry.interactionId, 1_000, 'interrupted'));
    } else if (answer.length > 0) {
      frames.push(completedFrame(entry.interactionId, answer));
    }
    return frames;
  }, []);

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
    ): { turn: TurnState; jobs: JobRow[] } => {
      if (frames !== null && frames.length > 0) {
        return projectFrames(
          entry.interactionId,
          frames,
          factsRef.current.get(entry.interactionId),
          readingsRef.current.get(entry.interactionId),
          entry.elapsedMs ?? null,
          truncated,
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
      const settled =
        turn.status === 'streaming' || turn.status === 'pending'
          ? settleTurn(turn, 'completed', entry.elapsedMs ?? null)
          : turn;
      /*
       * 索引读数先落到这一轮上：首字、速率与用量在帧回来之前就要在指标条上成立。
       * 窗口占用不在索引里，因此这一轮只有 token 分桶，上下文段仍等收尾负载。
       */
      const seeded = seedTurnReadings(settled, indexReadings(entry.interactionId));
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
              summaryFromToolNames(entry.toolNames ?? []),
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
    (entries: readonly HistoryEntry[]) => entries.map((entry) => projectTurn(entry, null).turn),
    [projectTurn],
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
      const generation = generationRef.current;
      setPageLoading(true);
      try {
        const entries = await fetchNewestPage(laneId);
        if (generation !== generationRef.current) return;
        factsRef.current = new Map();
        readingsRef.current = new Map();
        seedHistoryFacts(entries);
        rememberOldest(entries[0], entries.length >= HISTORY_PAGE_SIZE);
        dispatch({ type: 'reset' });
        if (entries.length > 0) {
          dispatch({ type: 'prepend-turns', turns: withProvisional(entries) });
        }
        const newest = entries[entries.length - 1];
        if (newest === undefined) return;
        // 最新一轮读整轮：它是正在读的那一段，思考与工具必须齐全并保持展开。
        const read = await readInteractionFrames(newest.interactionId, generation, {
          generation: () => generationRef.current,
        });
        if (read !== null && generation === generationRef.current) {
          const projected = projectTurn(newest, read.frames, read.truncated);
          dispatch({
            type: 'replace-turn',
            turn: projected.turn,
            jobs: projected.jobs,
          });
          if (!read.settled && workspaceId !== null) {
            void subscribeTail(newest.interactionId, read.cursor);
          }
        }
      } finally {
        setPageLoading(false);
      }
    },
    [
      dispatch,
      fetchNewestPage,
      generationRef,
      projectTurn,
      rememberOldest,
      seedHistoryFacts,
      subscribeTail,
      withProvisional,
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
      const entries = await fetchNewestPage(sessionId);
      if (generation !== generationRef.current) return;
      seedHistoryFacts(entries);
      const known = new Set(knownTurnIds());
      const fresh = entries.filter((entry) => !known.has(entry.interactionId));
      if (fresh.length === 0) return;
      dispatch({ type: 'append-turns', turns: withProvisional(fresh) });
      const newest = fresh[fresh.length - 1];
      const read = await readInteractionFrames(newest.interactionId, generation, {
        generation: () => generationRef.current,
      });
      if (read !== null && generation === generationRef.current) {
        const projected = projectTurn(newest, read.frames, read.truncated);
        dispatch({
          type: 'replace-turn',
          turn: projected.turn,
          jobs: projected.jobs,
        });
        if (!read.settled) void subscribeTail(newest.interactionId, read.cursor);
      }
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
    subscribeTail,
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
            read.truncated,
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
