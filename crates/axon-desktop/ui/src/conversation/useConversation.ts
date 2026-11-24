/*
 * 会话编排：把 IPC、订阅流与归约器接成一条链路。
 *
 * 与渲染解耦：本模块只负责「取数」与「派发增量」，组件只消费归约后的投影。
 * 帧到达后先在队列里攒一帧的窗口，再一次性派发，避免逐 token 触发渲染。
 *
 * **一个控制器服务两种目标**：主会话车道（按 Interaction 订阅）与子代理会话
 * （按会话订阅）。订阅、帧合并、失败语义与连接状态机在本文件里只有一份；
 * 目标差异全部收敛到 `normalizeTarget`（归一化）与 `sessionCapability`（车道能力门）两处。
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
import { SESSION_TURNS_PREFIX } from './sessionTurns';
import { useStreamFrames } from './useStreamFrames';
import { useTurnHeadRefresh } from './useTurnHeadRefresh';
import type { SessionEffectiveModel } from './viewConfig';
import { normalizeTarget } from './targets';
import type {
  ConversationController,
  ConversationTarget,
  SessionCapability,
} from './targets';
import { useAutoFoldTurns } from './useAutoFoldTurns';
import { useConversationInterruptControl } from './useConversationInterruptControl';
import { useElapsedTick } from './useElapsedTick';
import { useSessionPaging } from './useSessionPaging';
import { useSessionSync, type SessionSyncFacts } from './useSessionSync';
import { useQuestionSync } from './useQuestionSync';
import { useSessionCumulative } from './useSessionCumulative';
import type { SessionFacts } from './sessionFacts';

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
export type { ConversationController, ConversationTarget, SessionSelection } from './targets';
export { targetOfSession } from './targets';

/**
 * 组装会话控制器；`target` 为空表示还没选定会话，`sessionFacts` 见 conversation/useSessionSync。
 */
export function useConversation(
  target: ConversationTarget | null,
  sessionFacts: SessionSyncFacts | null = null,
): ConversationController {
  // 目标对象的身份每次渲染都可能变，因此只在「归一化后的稳定串」上做依赖。
  const normalized = target === null ? null : normalizeTarget(target);
  const workspaceId = normalized?.workspaceId ?? null;
  /**
   * 目标自身的稳定标识，effect 由它触发重跑。
   *
   * 不能只看 workspace 与 kind：同一工作区内切换会话时两者都不变，effect 不重跑，
   * 投影就会留着上一条会话的轮次（实测表现为「切换会话还是渲染上一个会话的内容」）。
   */
  const targetKey = target === null ? null : target.interactionId;
  /** 目标快照：effect 只在稳定串上重跑，但提交需要拿到当下的 destination。 */
  const targetRef = useRef<ConversationTarget | null>(target);
  targetRef.current = target;
  /**
   * 能力事实的回调必须**引用稳定**。
   *
   * 它会经 `fetchNewestPage` → `openSession` → `paging` 一路进入目标切换 effect 的依赖表；
   * 每渲染新建一次，那个 effect 就会在每次渲染后重跑，而它首行又派发一次状态，
   * 于是形成自激循环。当前目标因此用 ref 读取，依赖表保持为空。
   */
  const capabilityTargetRef = useRef<string | null>(targetKey);
  capabilityTargetRef.current = targetKey;
  const receiveSessionFacts = useCallback((facts: SessionFacts | null) => {
    setCapabilityFacts(facts);
    setCapabilityTarget(capabilityTargetRef.current);
    setCapabilityLoaded(true);
  }, []);
  /** 打开失败：能力事实从未到达，退回「正在读取」而不是判成「会话还没有登记」。 */
  const markSessionFactsPending = useCallback(() => {
    setCapabilityTarget(capabilityTargetRef.current);
    setCapabilityLoaded(false);
  }, []);

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
   * 换会话时投影继续持有上一屏，直到 `openSession` 把新一页连同重置一起落地：
   * 中间不出现空投影，车窗也就没有「先塌再长」的那一下。
   */

  const [state, dispatch] = useReducer(conversationReducer, INITIAL_CONVERSATION);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [questionError, setQuestionError] = useState<string | null>(null);
  const dismissQuestionError = useCallback(() => setQuestionError(null), []);
  const [sessionReadyId, setSessionReadyId] = useState<string | null>(null);
  /**
   * 车道当前模型的权威事实。
   *
   * 由网关按「该车道最新一轮记录」单条读出，随首屏历史一起回来；因此打开会话时
   * 输入坞立刻能显示真实模型，不必等逐条回放，也不会被旧轮次的帧改写。
   */
  const [sessionModel, setSessionModel] = useState<SessionEffectiveModel | null>(null);
  /**
   * 最近一次记忆反思提示。
   *
   * 与 `sessionModel` 同源同寿命：换目标时一起清空，翻页时不回退。反思是轮后异步治理
   * 的产物，界面按「上一次记忆动作」呈现，不承诺与当前轮同步。
   */
  const [sessionReflectionNotice, setSessionReflectionNotice] = useState<string | null>(null);
  /**
   * 会话能力事实。
   *
   * 与模型、反思提示同一生命周期：随首页历史索引一起取回，换目标即清空。可写性判断
   * 只认它（以及入口声明的只读），不再按来源渠道推断。
   */
  const [capabilityFacts, setCapabilityFacts] = useState<SessionFacts | null>(null);
  /** 能力事实是否已经取回：与「事实为空」分开，后者表示会话行不存在。 */
  const [capabilityLoaded, setCapabilityLoaded] = useState(false);
  /**
   * 能力事实所属的目标。
   *
   * 换会话时新旧目标在同一帧里交替，若只按「对象是否为空」判断，会先用上一条会话的
   * 能力位渲染一帧（例如从只读会话切到可写会话时，输入坞短暂仍是灰的）。因此把事实与
   * 目标一起记，读的时候要求两者一致。
   */
  const [capabilityTarget, setCapabilityTarget] = useState<string | null>(null);

  /**
   * 正文装载中的镜像：目标刚打开，或一页历史还没落地。
   *
   * 工作状态行据此写「正在加载会话…」。用状态而不是只读 ref：这一位要参与渲染，
   * 回放落地那一刻文案就得跟着换，而 ref 的变化不会触发重渲染。
   */
  const [replaying, setReplaying] = useState(false);
  const watchedRef = useRef<string | null>(null);
  /**
   * 目标代次。
   *
   * 回放与订阅都在目标切换之后才落地；没有代次围栏时，上一次目标的回放会在新目标下
   * 继续读旧 Interaction，或把旧订阅留在集合里。只有当前代次的调用才允许落地。
   */
  const generationRef = useRef(0);
  /** 归约器当前轮次快照；提交时需要判断是否处于运行中，但不能因此改变回调身份。 */
  const turnsRef = useRef(state.turns);
  turnsRef.current = state.turns;
  /**
   * 本控制器当前持有订阅的**全部**订阅标识。
   *
   * 帧过滤的判据只能是这个集合：历史回放按 Interaction 订阅，与车道标识、与选中的
   * interactionId 都不同。集合内的增删与真正的订阅/取消订阅一一对应。
   */
  const subscribedIdsRef = useRef<Set<string>>(new Set());
  const rememberSubscribed = useCallback((subscribeId: string) => {
    subscribedIdsRef.current.add(subscribeId);
  }, []);
  const rememberUnsubscribed = useCallback((subscribeId: string) => {
    subscribedIdsRef.current.delete(subscribeId);
  }, []);
  /** 当前会话标识（历史索引键）；首屏为空。子代理详情另给会话键，不能拿交互标识兼任。 */
  const pagingSessionId = target?.interactionId ?? null;

  /**
   * 主投影当前持有的轮次标识。
   *
   * 交给分页层做增量接管的判据：只有本视图还不认识的轮次才需要接进来，
   * 已经在投影里的轮次重放一遍不会带来任何新内容。
   */
  const knownTurnIds = useCallback(() => turnsRef.current.map((turn) => turn.id), []);
  /** 仍有轮次在跑时接上它的实时尾：历史帧只到取帧那一刻，之后是增量。 */
  const subscribeTail = useCallback(
    async (interactionId: string, cursor: string | null) => {
      const workspace = targetRef.current?.workspaceId ?? null;
      if (workspace === null) return;
      rememberSubscribed(`interaction:${interactionId}`);
      watchedRef.current = interactionId;
      await ipc.conversationWatch(workspace, interactionId, cursor);
    },
    [rememberSubscribed],
  );
  /*
   * 分页与展开：一页历史整页回放后一次性接进正文（见 useSessionPaging）。
   */
  const paging = useSessionPaging({
    sessionId: pagingSessionId,
    workspaceId: target?.workspaceId ?? null,
    dispatch,
    generationRef,
    onError: setSubmitError,
    knownTurnIds,
    subscribeTail,
    onEffectiveModel: setSessionModel,
    onReflectionNotice: setSessionReflectionNotice,
    onSessionFacts: receiveSessionFacts,
    onSessionFactsPending: markSessionFactsPending,
  });

  /**
   * 外部发起的新一轮：只把新增的轮次接进投影。
   *
   * 跨渠道事件（后台作业唤醒轮、定时投递、目标推进、子代理落地、问答续跑）到达时，
   * 这里不重置投影、也不重放已有历史：正文原地延长，滚动位置与已展开的过程都不动。
   * 重置一次就是整屏塌一次，那正是「来一条外部事件闪一下」的来源。
   */
  const appendExternalTurn = useCallback(() => {
    void paging.appendSession();
  }, [paging]);

  /** 释放本控制器当前持有的全部订阅；返回的 Promise 在退订送达后落地（失败不阻断）。 */
  const releaseAllSubscriptions = useCallback(async () => {
    const subscribeIds = [...subscribedIdsRef.current];
    subscribedIdsRef.current.clear();
    await Promise.all(subscribeIds.map(ipc.conversationUnsubscribe)).catch(() => undefined);
  }, []);

  /**
   * 轮次头刷新：把「多了一轮」翻译成一次索引重读。
   *
   * 走 ref 而不是直接闭包，是为了让帧回调保持引用稳定：它挂在流监听上，换一次身份就
   * 要重装监听。真正的重读由 `appendSession` 做，它按已加载的轮次标识去重，因此
   * 轮次头与轮数轮询同时命中也不会把同一轮接两遍。
   */
  const {
    handle: handleTurnHeadFrame,
    reset: resetTurnHead,
    clearTimers: clearTurnHeadTimers,
  } = useTurnHeadRefresh({
    targetRef,
    knownTurnIds: () => turnsRef.current.map((turn) => turn.id),
    appendSession: () => {
      void paging.appendSession();
    },
    dispatch,
  });
  /**
   * 帧到达：过滤、合批、按序号续读都在 `useStreamFrames` 里。
   *
   * 这一段整块搬出编排文件：它只处理订阅通道的三件事（属于谁、什么时候派发、
   * 截断后从哪里续），与正文投影无关；留在流程中间既推长本文件，也让「续读从哪开始」
   * 这条规则埋在别处。
   */
  const { onStreamEvent, resetFrames } = useStreamFrames({
    workspaceId,
    generationRef,
    dispatch,
    isSubscribed: (subscribeId) => subscribedIdsRef.current.has(subscribeId),
    handleTurnHeadFrame,
    forgetSubscription: rememberUnsubscribed,
    watchedRef,
  });

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

  /**
   * 目标变更：按目标建立订阅；车道目标再取最近一页并整页回放。
   *
   * 这里**不清空投影**。换会话时上一屏要一直显示到新一页落地（见设计系统 5.6）：
   * 先塌成空画布再长出来读起来就是切换那一下闪烁。重置与首屏数据在
   * `paging.openSession` 里同一次落地，正文因此只经历一次整屏替换。
   */
  useEffect(() => {
    // 开新代次：上一次目标遗留的回放与订阅据此全部作废。
    generationRef.current += 1;
    setReplaying(true);
    watchedRef.current = null;
    resetFrames();
    subscribedIdsRef.current = new Set();
    dispatch({ type: 'connecting' });
    setSubmitError(null);
    setSessionReadyId(null);
    setSessionModel(null);
    setSessionReflectionNotice(null);
    setCapabilityFacts(null);
    setCapabilityLoaded(false);
    // 换目标即重置轮次头的重试预算：上一条会话的失败次数不该压到这一条上。
    resetTurnHead();
    const current = targetRef.current;
    if (
      workspaceId === null ||
      targetKey === null ||
      current === null ||
      current.workspaceId !== workspaceId
    ) {
      // 没有可订阅的目标（首屏）：旧投影到此为止，不留上一轮的正文。
      setReplaying(false);
      dispatch({ type: 'reset' });
      return undefined;
    }
    const sessionId = current.interactionId;
    // 车道标识不是 Interaction：正文由这一页里的每条 Interaction 的 durable 帧给出。
    /*
     * 本机草稿还没有任何服务端事实：一条历史都不该去读。
     *
     * 按「空会话已就绪」收口，但**不能**把 `sessionReadyId` 标成草稿标识：那一格是
     * 「可以往这条会话发消息了」的判据，草稿恰恰还没有服务端会话可以写。标上它会让
     * 首条消息在物化完成之前就发出去，结果只换来一次「会话还没建立」并把正文丢掉。
     */
    if (current.draft === true) {
      dispatch({ type: 'reset' });
      setReplaying(false);
      setOpening(false);
      return undefined;
    }
    /*
     * 轮次头先于索引建立：先订阅再读索引，两者之间新建的那一轮不会漏；订阅首读只建
     * 基线，因此历史轮不会被当成新轮再播一遍。
     */
    const turnsId = `${SESSION_TURNS_PREFIX}${sessionId}`;
    rememberSubscribed(turnsId);
    void ipc.conversationWatchTurns(workspaceId, sessionId).catch(() => undefined);
    let cancelled = false;
    void paging
      .openSession(sessionId)
      .then(() => {
        if (cancelled) return;
        setReplaying(false);
        setOpening(false);
        setSessionReadyId(sessionId);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setReplaying(false);
        setSubmitError(error instanceof Error ? error.message : '读取会话历史失败');
        setOpening(false);
      });
    return () => {
      cancelled = true;
      // 轮次头的定时器属于这一代目标：切走后不再触发过期动作。
      clearTurnHeadTimers();
      // 实时续聊的订阅也属于当前视图，离开时全部释放，重新进入才能从头回放。
      void releaseAllSubscriptions();
    };
  }, [
    dispatch,
    paging.openSession,
    releaseAllSubscriptions,
    rememberSubscribed,
    rememberUnsubscribed,
    workspaceId,
    targetKey,
  ]);

  /** 运行中按节拍刷新耗时：没有轮次在跑时不起表。 */
  const ticking = state.turns.some(
    (turn) => turn.status === 'pending' || turn.status === 'streaming',
  );
  useElapsedTick(dispatch, ticking);
  /** 是否有轮次在跑：与节拍同源，避免同一判断写两遍。 */
  const working = ticking;

  // 外部发起的新一轮（后台作业唤醒轮、定时投递、目标推进）靠会话轮数变化补齐接管。
  useSessionSync({
    sessionFacts,
    ready: sessionReadyId === targetKey && !opening,
    working,
    onReattach: appendExternalTurn,
  });

  // 折叠生效：最新一轮之外的老轮次撤下过程、只留一根过程控件（见 useAutoFoldTurns）。
  useAutoFoldTurns(state.turns, dispatch);

  /**
   * 过渡窗口的上限：兜底收口，正文区不会一直停在过渡面上。
   *
   * 一页回放本身可能在途（整页读完才落地），那段时间不算卡住，计时让位给它；
   * 读失败或代次作废时由调用方把 `opening` 收掉。
   */
  useEffect(() => {
    if (!opening || paging.pageLoading) return undefined;
    const timer = window.setTimeout(() => setOpening(false), OPENING_CEILING_MS);
    return () => window.clearTimeout(timer);
  }, [opening, paging.pageLoading]);

  const sessionCapability = useMemo<SessionCapability>(
    () =>
      target === null
        ? { available: false, sessionId: null, workspaceId: null, destination: null }
        : {
            available: true,
            sessionId: target.interactionId,
            workspaceId: target.workspaceId,
            destination: target.destination,
          },
    [target],
  );

  const submit = useCallback(
    async (
      text: string,
      attachments: AttachmentRef[] = [],
      structuredInput = '',
      model: string | null = null,
      reasoningEffort: string | null = null,
    ) => {
      // 会话目标不支持在这里发送：如实失败，不静默丢弃。
      if (!sessionCapability.available) {
        const message = '该会话不支持在这里发送消息';
        setSubmitError(message);
        throw new Error(message);
      }
      // 服务端会把正文首尾的空白裁掉：界面若留着它，回放就认领不上这一格，
      // 同一条消息会在时间线上多出一个永远等不到回复的气泡。
      const visibleText = text.trim();
      const workspace = sessionCapability.workspaceId ?? '';
      const destination = sessionCapability.destination ?? '';
      setSubmitting(true);
      setSubmitError(null);
      setReplaying(false);
      // 提交是用户自己的动作：正文立刻可见，不为它保留过渡窗口。
      setOpening(false);
      const nowMs = Date.now();
      const streaming = turnsRef.current.some(
        (turn) => turn.status === 'pending' || turn.status === 'streaming',
      );
      const localTurnId = `pending-${nowMs}`;
      if (streaming) {
        // 运行中：注入当前轮次排队，等安全边界插入，不新开一轮。
        dispatch({ type: 'steer', text: visibleText, nowMs, attachments, structuredInput });
      } else {
        dispatch({ type: 'submit', turnId: localTurnId, text: visibleText, nowMs, attachments });
      }
      try {
        const receipt = await ipc.conversationSubmit(
          workspace,
          sessionCapability.sessionId,
          destination,
          // 结构化输入只进入发出去的正文：正文气泡仍显示用户自己敲的那一段。
          structuredInput.length > 0 ? `${visibleText}${structuredInput}` : visibleText,
          attachments.map((attachment) => attachment.path),
          model,
          reasoningEffort,
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
    [sessionCapability, rememberSubscribed],
  );

  const retry = useCallback(() => {
    const sessionId = sessionCapability.sessionId;
    const workspace = sessionCapability.workspaceId;
    if (sessionId === null || workspace === null) return;
    // 重连第一步是放手旧订阅：订阅断掉后后端那一格仍占着该标识，不退订就重放，
    // 回放里那次重新订阅会被幂等挡下，界面拼完历史也收不到增量。
    resetFrames();
    watchedRef.current = null;
    setSessionReadyId(null);
    // 重连重开一轮：清掉上一次的定时器，并把重试预算复位。
    clearTurnHeadTimers();
    resetTurnHead();
    // 重连走的也是同一条回放：同样在过渡面下把正文拼齐，再一次交出。
    setOpening(true);
    void releaseAllSubscriptions()
      .then(() => {
        // 轮次头随正文一起重建：重连之后界面仍要能发现新轮。
        rememberSubscribed(`${SESSION_TURNS_PREFIX}${sessionId}`);
        return ipc.conversationWatchTurns(workspace, sessionId).catch(() => undefined);
      })
      .then(() => paging.openSession(sessionId))
      .then(() => {
        setSessionReadyId(sessionId);
        setOpening(false);
      })
      .catch((error: unknown) => {
        setSubmitError(error instanceof Error ? error.message : '重连失败');
        setOpening(false);
      });
  }, [sessionCapability, paging.openSession, releaseAllSubscriptions, rememberSubscribed]);

  /*
   * 中断与转投：三步的先后顺序本身就是语义，因此整条装配收在
   * `useConversationInterruptControl` 里，本文件只保留一个 `interrupt` 出口。
   */
  const interrupt = useConversationInterruptControl({
    sessionId: sessionCapability.sessionId,
    watchedRef,
    targetKey,
    working,
    turnsRef,
    dispatch,
    submit,
    onNotice: setSubmitError,
  });

  /**
   * 接管网关语音回合：把这一轮接进当前对话流并订阅增量。
   *
   * 语音的定稿由网关经既有入口提交，界面在此之前并不知道这条 Interaction；
   * 若不接管，用户说完话只能看到草稿里多了一段字，而回复其实已经在跑。
   * 因此收到 `turn` 帧就补一条本地轮次并绑上它的 interactionId，
   * 后续增量与收口都走与文本输入完全相同的订阅通道。
   */
  const attachVoiceTurn = useCallback(
    async (interactionId: string, utterance: string) => {
      const workspace = sessionCapability.workspaceId ?? '';
      if (!sessionCapability.available || interactionId.length === 0) return;
      const visibleText = utterance.trim();
      const nowMs = Date.now();
      const localTurnId = `voice-${nowMs}-${Math.random().toString(36).slice(2, 7)}`;
      dispatch({ type: 'submit', turnId: localTurnId, text: visibleText, nowMs, attachments: [] });
      dispatch({ type: 'bind', turnId: localTurnId, interactionId });
      if (watchedRef.current !== interactionId) {
        watchedRef.current = interactionId;
        rememberSubscribed(`interaction:${interactionId}`);
        await ipc.conversationWatch(workspace, interactionId, null);
      }
    },
    [sessionCapability, rememberSubscribed],
  );

  const { activeQuestion, answerQuestion } = useQuestionSync({
    sessionId: sessionCapability.sessionId,
    state,
    dispatch,
    onError: setQuestionError,
    onContinuation: appendExternalTurn,
  });

  const jobs = useMemo(() => deriveJobs(state), [state]);
  const queued = useMemo(() => pendingSteerCount(state.turns), [state.turns]);

  /**
   * 对外的投影。
   *
   * 直接交出归约器状态：换会话期间它仍是上一屏，新一页落地时与重置同批替换，
   * 因此调用方只会看到一次替换，不会先看到一屏空的。
   */
  const projection = state;

  /** 会话累计读数：网关基线加本地增量；基线未就绪时为空，指标条不画这两段。 */
  // 累计读数按当前会话核对该基线：换目标之后才落地的旧页属于上一条会话，不能拿来画这一条。
  const sessionCumulative = useSessionCumulative(
    projection.turns,
    paging.sessionBaseline,
    targetKey,
  );

  /**
   * 加载更早的一页轮次。
   *
   * 轮内不再提供「往前补」：补进来的正文会立刻被同一个渲染预算折掉，等于白读一窗帧。
   * 一轮里更早的部分只由时间线上那句「更早的 N 条内容已折叠」说明。
   */
  const loadOlder = useCallback(async () => {
    await paging.loadOlder();
  }, [paging]);

  return {
    state: projection,
    jobs,
    working,
    /** 正文装载中：工作状态行写「正在加载会话…」而不是供应商阶段。 */
    loadingHistory: opening || replaying,
    /** 正在展开的折叠轮：过程控件据此把箭头换成转圈。 */
    unfoldingId: paging.unfoldingId,
    activeQuestion,
    answerQuestion,
    watchedInteractionId: sessionCapability.available ? watchedRef.current : null,
    sessionCumulative,
    sessionReadyId,
    /** 车道当前模型的权威事实：网关只读该车道最新一轮记录。 */
    sessionModel,
    sessionReflectionNotice,
    /** 会话能力事实：可写性、可中断与可答问；只认归当前目标的那一份。 */
    sessionFacts: capabilityTarget === targetKey ? capabilityFacts : null,
    /** 能力事实是否已经取回过（`null` 也算取回过，代表会话行不存在）。 */
    sessionFactsLoaded: capabilityTarget === targetKey && capabilityLoaded,
    sessionAvailable: sessionCapability.available,
    opening,
    queued,
    submitting,
    submitError,
    questionError,
    dismissQuestionError,
    submit,
    attachVoiceTurn,
    retry,
    interrupt,
    loadOlder,
    hasOlder: paging.hasOlder,
    loadingOlder: paging.loadingOlder,
    prependToken: paging.prependToken,
    unfoldTurn: paging.unfoldTurn,
    collapseTurn: (turnId: string) => dispatch({ type: 'collapse-turn', turnId }),
    /**
     * 按服务端判据收口这一轮。
     *
     * 运行证据判过期之后走这里：状态、计时与结论行都在归约器里落定，
     * 视图只负责把服务端的 verdict 交进来。
     */
    settleSilentTurn: (interactionId: string, verdict: string) =>
      dispatch({ type: 'settle-silent-turn', interactionId, verdict, nowMs: Date.now() }),
    clearSubmitError: () => setSubmitError(null),
  };
}
