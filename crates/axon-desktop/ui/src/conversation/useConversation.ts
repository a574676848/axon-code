/*
 * 会话编排：把 IPC、订阅流与归约器接成一条链路。
 *
 * 与渲染解耦：本模块只负责「取数」与「派发增量」，组件只消费归约后的投影。
 * 帧到达后先在队列里攒一帧的窗口，再一次性派发，避免逐 token 触发渲染。
 *
 * **一个控制器服务两种目标**：主会话车道（按 Interaction 订阅）与子代理会话
 * （按会话订阅）。订阅、帧合并、失败语义与连接状态机在本文件里只有一份；
 * 目标差异全部收敛到 `normalizeTarget`（归一化）与 `laneCapability`（车道能力门）两处。
 */

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react';
import * as ipc from '../ipc';
import {
  INITIAL_CONVERSATION,
  conversationReducer,
  deriveJobs,
  pendingSteerCount,
  type AttachmentRef,
} from './transcript';
import type { ConversationStreamEvent } from './wire';
import { normalizeTarget, subscriptionIdOf } from './targets';
import type {
  ConversationController,
  ConversationTarget,
  LaneCapability,
} from './targets';
import { useAutoFoldTurns } from './useAutoFoldTurns';
import { useElapsedTick } from './useElapsedTick';
import { useLanePaging } from './useLanePaging';
import { useLaneReattach } from './useLaneReattach';
import { useQuestionSync } from './useQuestionSync';

/** 一帧的合并窗口，与 TUI 的助手增量归并窗口同量级。 */
const FRAME_FLUSH_MS = 16;
/**
 * 过渡窗口的上限。
 *
 * 收口正常由回放排空触发；这一条只兜底：回放卡在等终态帧时，正文区不能一直停在
 * 过渡面上。
 */
const OPENING_CEILING_MS = 900;
/**
 * 目标与契约类型整块移到 `targets.ts`：它们是纯类型与纯函数，编排文件只负责流程。
 * 这里原样再导出，调用方的引用不必改。
 */
export type { ConversationController, ConversationTarget, LaneSelection } from './targets';
export { targetOfLane } from './targets';

/** 组装会话控制器；`target` 为空表示还没有选定的会话。 */
export function useConversation(target: ConversationTarget | null): ConversationController {
  // 目标对象的身份每次渲染都可能变，因此只在「归一化后的稳定串」上做依赖。
  const normalized = target === null ? null : normalizeTarget(target);
  const workspaceId = normalized?.workspaceId ?? null;
  const targetKind = normalized?.kind ?? null;
  /**
   * 目标自身的稳定标识，effect 由它触发重跑。
   *
   * 不能只看 workspace 与 kind：同一工作区内切换会话时两者都不变，effect 不重跑，
   * 投影就会留着上一条会话的轮次（实测表现为「切换会话还是渲染上一个会话的内容」）。
   */
  const targetKey =
    target === null
      ? null
      : target.kind === 'session'
        ? target.sessionId
        : target.interactionId;
  /** 目标快照：effect 只在稳定串上重跑，但提交需要拿到当下的 destination。 */
  const targetRef = useRef<ConversationTarget | null>(target);
  targetRef.current = target;

  /** 已经进入过渡窗口的目标；与当前目标不一致的那一帧就是切换那一帧。 */
  const [openedTarget, setOpenedTarget] = useState<string | null>(targetKey);
  const [opening, setOpening] = useState(false);
  if (targetKey !== openedTarget) {
    /*
     * 与目标切换同一次提交进入过渡窗口。
     *
     * 晚一帧就会先画出「上一屏已经作废、新内容还没到」的空档，而那正是切换时
     * 第一下闪烁。渲染期改自身状态不会提交中间结果，React 会立刻重渲染。
     */
    setOpenedTarget(targetKey);
    setOpening(targetKey !== null);
  }
  /**
   * 投影所属的目标。
   *
   * 归约器的重置落在目标 effect 里（提交之后），因此切换那一帧 state 里还是上一会话
   * 的轮次。按这个标识对齐后，旧投影没有任何机会被画出去。
   */
  const [projectionKey, setProjectionKey] = useState<string | null>(null);

  const [state, dispatch] = useReducer(conversationReducer, INITIAL_CONVERSATION);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [questionError, setQuestionError] = useState<string | null>(null);
  const dismissQuestionError = useCallback(() => setQuestionError(null), []);
  const [laneReadyId, setLaneReadyId] = useState<string | null>(null);
  const laneReattach = useLaneReattach();

  const pendingRef = useRef<ConversationStreamEvent[]>([]);
  const flushRef = useRef<number | null>(null);
  const queueRef = useRef<string[]>([]);
  const replayingRef = useRef(false);
  const watchedRef = useRef<string | null>(null);
  /**
   * 目标代次。
   *
   * 回放推进既由帧触发（帧窗口回调）也由空闲计时器触发，两者都会在目标切换之后
   * 才落地；没有代次围栏时，上一次目标的回放会在新目标下继续订阅旧 Interaction，
   * 于是每切一次会话就多留几路永不释放的订阅。只有当前代次的调用才允许订阅。
   */
  const generationRef = useRef(0);
  /** 归约器当前轮次快照；提交时需要判断是否处于运行中，但不能因此改变回调身份。 */
  const turnsRef = useRef(state.turns);
  turnsRef.current = state.turns;
  /**
   * 本控制器当前持有订阅的**全部**订阅标识。
   *
   * 帧过滤的判据只能是这个集合，不能是「当前选中的车道」：车道历史是逐条回放的，
   * 订阅标识属于被回放的那条 Interaction，与车道标识、与选中的 interactionId 都不同。
   * 集合内的增删与真正的订阅/取消订阅一一对应。
   */
  const subscribedIdsRef = useRef<Set<string>>(new Set());
  const rememberSubscribed = useCallback((subscribeId: string) => {
    subscribedIdsRef.current.add(subscribeId);
  }, []);
  const rememberUnsubscribed = useCallback((subscribeId: string) => {
    subscribedIdsRef.current.delete(subscribeId);
  }, []);
  /**
   * 回放推进与空闲停止的最新实现。
   *
   * 帧监听只装一次（目标切换不重建监听，否则切换瞬间的帧会掉进空档），
   * 因此它必须经 ref 取当前实现，而不是把回调重新绑进 effect 依赖。
   * `playNext` 的第三个参数是「即将离开的那条 Interaction」；为空表示保留它的订阅。
   */
  const playNextRef = useRef<
    (workspaceId: string, generation: number, leaving?: string | null) => void
  >(() => undefined);
  /** 中断请求是否在途：Esc 连发与长按只允许换来一次真实中断。 */
  const interruptingRef = useRef(false);
  /** 当前车道与工作区：会话目标下没有车道能力，分页与展开也随之不可用。 */
  const pagingLaneId = target !== null && target.kind === 'interaction' ? target.interactionId : null;

  /*
   * 分页与展开：与主回放共用代次与在途标记，但自己持有临时投影（见 useLanePaging）。
   */
  const paging = useLanePaging({
    laneId: pagingLaneId,
    workspaceId: target?.workspaceId ?? null,
    dispatch,
    replayingRef,
    generationRef,
    playNextRef,
    onError: setSubmitError,
  });

  /**
   * 回放一条 Interaction 的正文：历史走 HTTP 帧页，读完只订阅实时尾。
   *
   * 历史不再逐帧穿过事件通道：通道上因此只出现真正的增量，大会话的上万帧也不会
   * 再把 WebView 压垮。分页逐页落地，正文随页码推进逐步长出，不会先空一屏。
   */
  const replayHistory = useCallback(
    async (workspace: string, interactionId: string, generation: number) => {
      const subscribeId = `interaction:${interactionId}`;
      let after = 0;
      let tailCursor: string | null = null;
      let settled = false;
      for (;;) {
        const page = await ipc.conversationHistoryEvents(interactionId, after);
        if (generation !== generationRef.current) return;
        if (page.events.length > 0) {
          paging.routeFrames(
            page.events.map((event) => ({
              kind: 'frame' as const,
              subscribeId,
              eventType: event.eventType,
              payloadJson: event.payloadJson,
              cursor: event.cursor,
            })),
            Date.now(),
          );
          // 终态帧只会在补到 durable 尾的那一页出现：它同时说明这一条已经收口。
          settled = page.events.some(
            (event) =>
              event.eventType === 'turn_completed' ||
              event.eventType === 'turn_failed' ||
              event.eventType === 'interaction.status',
          );
        }
        tailCursor = page.tailCursor;
        after = page.nextAfter;
        if (!page.hasMore) break;
      }
      if (generation !== generationRef.current) return;
      if (settled) {
        // 已经收口：不再订阅，直接把回放推进到下一条。
        playNextRef.current(workspace, generation, null);
        return;
      }
      // 还在跑：从 durable 尾订阅，事件通道上只会出现真正的增量。
      rememberSubscribed(subscribeId);
      await ipc.conversationWatch(workspace, interactionId, tailCursor);
    },
    [paging.routeFrames, rememberSubscribed],
  );

  const releaseInteraction = useCallback(
    (interactionId: string) => {
      const subscribeId = `interaction:${interactionId}`;
      if (watchedRef.current === interactionId) {
        watchedRef.current = null;
      }
      rememberUnsubscribed(subscribeId);
      void ipc.conversationUnsubscribe(subscribeId).catch(() => undefined);
    },
    [rememberUnsubscribed],
  );

  const playNext = useCallback(
    (workspace: string, generation: number, leaving: string | null = null) => {
      // 代次不符即作废：目标已切换，这一次回放属于上一次目标，不再建立任何订阅。
      //
      // 判据必须是代次而不是工作区：同一工作区内切换会话时工作区不变，
      // 只看工作区会把上一次目标的遗留计时器放进来，于是每切一次就多留几路
      // 永不释放的订阅（实测过：切三次会话订阅数从 2 涨到 5）。
      if (generation !== generationRef.current) {
        replayingRef.current = false;
        return;
      }
      if (leaving !== null) releaseInteraction(leaving);
      const next = paging.take(queueRef.current);
      if (next === undefined) {
        replayingRef.current = false;
        // 翻页或展开的一次回放排空：由分页接管收口（前置一页 / 换回正文）。
        if (paging.finish()) return;
        // 回放排空即正文完整：过渡窗口到此收口，正文一次交出。
        setOpening(false);
        return;
      }
      replayingRef.current = true;
      watchedRef.current = next;
      paging.applyAction({ type: 'open-turn', turnId: next, nowMs: Date.now() });
      const seed = paging.seedOf(next);
      if (seed !== undefined) {
        paging.applyAction({
          type: 'seed-history',
          interactionId: next,
          text: seed.text,
          attachments: seed.attachments,
        });
      }
      void replayHistory(workspace, next, generation).catch((error: unknown) => {
        if (generationRef.current !== generation) return;
        setSubmitError(error instanceof Error ? error.message : '读取会话历史失败');
        // 读不到正文也要继续推进，否则这一条会把整条车道的回放卡住。
        playNextRef.current(workspace, generation, watchedRef.current);
      });
    },
    [paging.applyAction, paging.seedOf, releaseInteraction, replayHistory],
  );
  playNextRef.current = playNext;

  /** 帧到达：按本控制器持有的订阅标识过滤，入队并在下一帧窗口一次性派发。 */
  const onStreamEvent = useCallback(
    (event: ConversationStreamEvent) => {
      // 一条连接承载多路订阅；只收属于自己的那几路，其余（别的视图的订阅）直接丢弃。
      if (!subscribedIdsRef.current.has(event.subscribeId)) return;
      pendingRef.current.push(event);
      if (flushRef.current !== null) return;
      const generation = generationRef.current;
      flushRef.current = window.setTimeout(() => {
        flushRef.current = null;
        // 目标已切换：这一批帧属于上一次目标，丢弃即可（新目标会重置投影）。
        if (generationRef.current !== generation) {
          pendingRef.current = [];
          return;
        }
        const batch = pendingRef.current;
        pendingRef.current = [];
        if (batch.length === 0) return;
        // 帧分流交给分页：翻页期间两路帧同时在跑，按订阅标识各归各的投影。
        paging.routeFrames(batch, Date.now());
        // 只有车道目标逐条回放历史；会话流由服务端一次重放到底。
        if (!replayingRef.current || targetKind !== 'interaction' || workspaceId === null) return;
        const terminal = batch.some(
          (frame) =>
            frame.kind === 'frame' &&
            (frame.eventType === 'turn_completed' || frame.eventType === 'turn_failed'),
        );
        // 收到终态帧才释放当前这一条：它已经不会再产生事件，订阅位应当还回去。
        if (terminal) playNextRef.current(workspaceId, generation, watchedRef.current);
      }, FRAME_FLUSH_MS);
    },
    [targetKind, workspaceId],
  );

  useEffect(() => {
    let dispose: (() => void) | null = null;
    let cancelled = false;
    void ipc
      .listenConversation(onStreamEvent)
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch((error: unknown) => {
        // 监听装不上就等于整条事件通道断了；这里必须显式暴露，
        // 否则界面只会停在「正在生成」而不给任何原因。
        if (cancelled) return;
        dispatch({
          type: 'frames',
          frames: [
            {
              kind: 'failure',
              subscribeId: subscribedIdsRef.current.values().next().value ?? '',
              message: `无法建立会话事件监听：${
                error instanceof Error ? error.message : String(error)
              }`,
              retryable: true,
            },
          ],
          nowMs: Date.now(),
        });
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [onStreamEvent]);

  /** 目标变更：重置投影，按目标建立订阅；车道目标再按时间顺序逐条回放历史 Interaction。 */
  useEffect(() => {
    // 开新代次：上一次目标遗留的帧窗口与回放推进据此全部作废。
    generationRef.current += 1;
    queueRef.current = [];
    replayingRef.current = false;
    watchedRef.current = null;
    pendingRef.current = [];
    subscribedIdsRef.current = new Set();
    dispatch({ type: 'reset' });
    dispatch({ type: 'connecting' });
    setSubmitError(null);
    setLaneReadyId(null);
    // 投影与目标同批对齐：从这一帧起，state 里的旧轮次不再对外可见。
    setProjectionKey(targetKey);
    const current = targetRef.current;
    if (
      workspaceId === null ||
      targetKind === null ||
      targetKey === null ||
      current === null ||
      normalizeTarget(current).kind !== targetKind ||
      normalizeTarget(current).workspaceId !== workspaceId
    ) {
      return undefined;
    }
    const watchedId = subscriptionIdOf(current);

    const report = (message: string, retryable: boolean) => {
      // 订阅失败要如实亮出来，不能停在过渡面上。
      setOpening(false);
      dispatch({
        type: 'frames',
        frames: [{ kind: 'failure', subscribeId: watchedId, message, retryable }],
        nowMs: Date.now(),
      });
    };

    if (current.kind === 'session') {
      rememberSubscribed(watchedId);
      void ipc
        .conversationWatchSession(workspaceId, current.sessionId, null)
        .catch((error: unknown) => {
          report(
            error instanceof Error ? error.message : '订阅子代理会话事件流失败',
            false,
          );
        });
      return () => {
        rememberUnsubscribed(watchedId);
        void ipc.conversationUnsubscribe(watchedId).catch(() => undefined);
      };
    }

    const laneId = current.interactionId;
    // 车道标识不是 Interaction；由历史索引逐条订阅真实交互，新消息按提交回执订阅。
    let cancelled = false;
    // 历史按「一次一条」向前翻：首屏只取最近一条，更早的由「加载更早」按需取。
    void paging
      .loadNewestPage(laneId)
      .then((ids) => {
        if (cancelled) return;
        queueRef.current = ids;
        // 已经收口的交互仍然要回放：正文的事实源就是订阅流上的 durable 事件。
        playNextRef.current(workspaceId, generationRef.current);
        setLaneReadyId(laneId);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setSubmitError(error instanceof Error ? error.message : '读取会话历史失败');
        setOpening(false);
      });
    return () => {
      cancelled = true;
      // 实时续聊的订阅也属于当前视图，离开时全部释放，重新进入才能从头回放。
      for (const subscribeId of subscribedIdsRef.current) {
        void ipc.conversationUnsubscribe(subscribeId).catch(() => undefined);
      }
      subscribedIdsRef.current.clear();
    };
  }, [
    rememberSubscribed,
    rememberUnsubscribed,
    workspaceId,
    targetKind,
    targetKey,
    laneReattach.token,
  ]);

  /** 运行中按节拍刷新耗时。 */
  useElapsedTick(dispatch);

  // 折叠生效：体量过大与超出窗口的老轮次折成一行摘要并释放正文（见 useAutoFoldTurns）。
  useAutoFoldTurns(state.turns, dispatch);

  /** 过渡窗口的上限：兜底收口，正文区不会一直停在过渡面上。 */
  useEffect(() => {
    if (!opening) return undefined;
    const timer = window.setTimeout(() => setOpening(false), OPENING_CEILING_MS);
    return () => window.clearTimeout(timer);
  }, [opening]);

  const laneCapability = useMemo<LaneCapability>(
    () =>
      target !== null && target.kind === 'interaction'
        ? {
            available: true,
            laneId: target.interactionId,
            workspaceId: target.workspaceId,
            destination: target.destination,
          }
        : { available: false, laneId: null, workspaceId: null, destination: null },
    [target],
  );

  const submit = useCallback(
    async (text: string, attachments: AttachmentRef[] = [], structuredInput = '') => {
      // 会话目标不支持在这里发送：如实失败，不静默丢弃。
      if (!laneCapability.available) {
        const message = '该会话不支持在这里发送消息';
        setSubmitError(message);
        throw new Error(message);
      }
      // 服务端会把正文首尾的空白裁掉：界面若留着它，回放就认领不上这一格，
      // 同一条消息会在时间线上多出一个永远等不到回复的气泡。
      const visibleText = text.trim();
      const workspace = laneCapability.workspaceId ?? '';
      const destination = laneCapability.destination ?? '';
      setSubmitting(true);
      setSubmitError(null);
      queueRef.current = [];
      replayingRef.current = false;
      // 提交是用户自己的动作：正文立刻可见，不为它保留过渡窗口。
      setOpening(false);
      const nowMs = Date.now();
      const streaming = turnsRef.current.some(
        (turn) => turn.status === 'pending' || turn.status === 'streaming',
      );
      const localTurnId = `pending-${nowMs}`;
      if (streaming) {
        // 运行中：注入当前轮次排队，等安全边界插入，不新开一轮。
        dispatch({ type: 'steer', text: visibleText, nowMs, attachments });
      } else {
        dispatch({ type: 'submit', turnId: localTurnId, text: visibleText, nowMs, attachments });
      }
      try {
        const receipt = await ipc.conversationSubmit(
          workspace,
          destination,
          // 结构化输入只进入发出去的正文：正文气泡仍显示用户自己敲的那一段。
          structuredInput.length > 0 ? `${visibleText}${structuredInput}` : visibleText,
          attachments.map((attachment) => attachment.path),
        );
        if (!streaming) {
          dispatch({ type: 'bind', turnId: localTurnId, interactionId: receipt.interactionId });
        }
        if (watchedRef.current !== receipt.interactionId) {
          watchedRef.current = receipt.interactionId;
          // 提交可能命中已在运行的那一条 Interaction；订阅它并登记，帧才会被收下。
          rememberSubscribed(`interaction:${receipt.interactionId}`);
          await ipc.conversationWatch(workspace, receipt.interactionId, null);
        }
      } catch (error) {
        const message = error instanceof Error ? error.message : '发送失败';
        setSubmitError(message);
        /*
         * 提交失败必须收回乐观状态：否则新开的一轮会永远停在「生成回复中」，
         * 排队的那一格会永远等着一次不会发生的注入。消息本体标成未送达留在
         * 正文里，用户看得见也点得到「重新发送」。
         */
        dispatch({
          type: 'submit-failed',
          turnId: streaming ? null : localTurnId,
          text: visibleText,
          message,
          nowMs: Date.now(),
        });
        throw error;
      } finally {
        setSubmitting(false);
      }
    },
    [laneCapability, rememberSubscribed],
  );

  const retry = useCallback(() => {
    const laneId = laneCapability.laneId;
    const workspace = laneCapability.workspaceId;
    if (laneId === null || workspace === null) return;
    pendingRef.current = [];
    dispatch({ type: 'reset' });
    setLaneReadyId(null);
    // 重连走的也是同一条回放：同样在过渡面下把正文拼齐，再一次交出。
    setOpening(true);
    void paging
      .loadNewestPage(laneId)
      .then((ids) => {
        queueRef.current = ids;
        // 重连发生在同一次目标内：代次不变，回放照常推进。
        playNextRef.current(workspace, generationRef.current);
        setLaneReadyId(laneId);
      })
      .catch((error: unknown) => {
        setSubmitError(error instanceof Error ? error.message : '重连失败');
      });
  }, [laneCapability, paging]);

  /** 中断当前车道正在运行的一轮；回执为假时如实提示，不假装成功。 */
  const interrupt = useCallback(async () => {
    const lane = laneCapability.laneId ?? watchedRef.current;
    if (lane === null) return;
    /*
     * 一次按键只发一次中断。
     *
     * Esc 长按会连发 keydown，而每次中断都是一次真实的后端请求；在途期间的重复
     * 请求只会换来「没有正在运行的一轮」这种噪声结论。
     */
    if (interruptingRef.current) return;
    interruptingRef.current = true;
    try {
      const receipt = await ipc.conversationInterrupt(lane);
      if (!receipt.interrupted) {
        setSubmitError('本轮已经结束，无需中断');
      }
    } catch (error) {
      setSubmitError(error instanceof Error ? error.message : '中断失败');
    } finally {
      interruptingRef.current = false;
    }
  }, [laneCapability.laneId]);

  const { activeQuestion, answerQuestion } = useQuestionSync({
    laneId: laneCapability.laneId,
    state,
    dispatch,
    onError: setQuestionError,
    onContinuation: laneReattach.reattach,
    reattachToken: laneReattach.token,
  });

  const jobs = useMemo(() => deriveJobs(state), [state]);
  const queued = useMemo(() => pendingSteerCount(state.turns), [state.turns]);
  const working = useMemo(
    () => state.turns.some((turn) => turn.status === 'pending' || turn.status === 'streaming'),
    [state.turns],
  );

  /**
   * 对外的投影。
   *
   * 只在与当前目标对齐之后才交出去：目标切换那一帧 state 里还是上一会话的轮次，
   * 直接画出去就是切换时的第一下闪烁。
   */
  const projection = projectionKey === targetKey ? state : INITIAL_CONVERSATION;

  return {
    state: projection,
    jobs,
    working,
    activeQuestion,
    answerQuestion,
    watchedInteractionId: laneCapability.available ? watchedRef.current : null,
    laneReadyId,
    laneAvailable: laneCapability.available,
    opening,
    queued,
    submitting,
    submitError,
    questionError,
    dismissQuestionError,
    submit,
    retry,
    interrupt,
    loadOlder: paging.loadOlder,
    hasOlder: paging.hasOlder,
    loadingOlder: paging.loadingOlder,
    prependToken: paging.prependToken,
    unfoldTurn: paging.unfoldTurn,
    clearSubmitError: () => setSubmitError(null),
  };
}
