/*
 * 帧批处理与按序号续读：订阅流的唯一落点。
 *
 * 一条连接承载多路订阅，这里只做三件事：
 *
 * 1. 过滤：只收本控制器持有的那几路订阅，其余视图的帧直接丢弃；
 * 2. 合批：在同一个 16ms 窗口里合并成一次归约（逐帧派发会让宿主跟不上）；
 * 3. 续读：确认帧说这次回放没追到 durable 尾时，按服务端给的真实游标重新订阅，
 *    并按订阅内的序号去重，重叠帧不会长出重复气泡。
 *
 * 续读只认游标、不从 0 重放：从 0 重放会把整段历史再叠一遍，而残缺段仍然缺。
 * 同一个游标只请求一次：否则一次真截断会被反复续订，界面永远停在「待补齐」。
 */

import { useCallback, useRef } from 'react';

import * as ipc from '../ipc';
import type { ConversationStreamEvent } from './wire';
import { acceptsFrameSequence, replayResume } from './replayTruncation';
import { cursorSequence } from './interactionReplay';
import type { ConversationAction } from './transcript';

/** 一帧的合并窗口，与 TUI 的助手增量归并窗口同量级。 */
const FRAME_FLUSH_MS = 16;

/** 续读与去重共用的订阅标识前缀。 */
const INTERACTION_PREFIX = 'interaction:';

export interface StreamFrameOptions {
  /** 当前目标的工作区；续订需要它（网关按工作区校验会话归属）。 */
  workspaceId: string | null;
  /** 目标代次：帧到达时与派发时都要核对，旧目标的回放不得落到新目标上。 */
  generationRef: React.MutableRefObject<number>;
  /** 归约入口。 */
  dispatch: (action: ConversationAction) => void;
  /** 这一路订阅是不是本控制器持有的。 */
  isSubscribed: (subscribeId: string) => boolean;
  /** 轮次头帧：返回 true 表示已消费，不进正文投影。 */
  handleTurnHeadFrame: (event: ConversationStreamEvent) => boolean;
  /** 订阅已经结束：从持有集合里摘掉。 */
  forgetSubscription: (subscribeId: string) => void;
  /** 正在订阅的那一条 Interaction。 */
  watchedRef: React.MutableRefObject<string | null>;
}

export interface StreamFrames {
  /** 事件通道的落点。 */
  onStreamEvent: (event: ConversationStreamEvent) => void;
  /** 换目标时丢弃还没派发的那一批。 */
  resetFrames: () => void;
}

export function useStreamFrames(options: StreamFrameOptions): StreamFrames {
  const { workspaceId, generationRef, dispatch, isSubscribed, handleTurnHeadFrame, forgetSubscription, watchedRef } =
    options;
  const pendingRef = useRef<ConversationStreamEvent[]>([]);
  const flushRef = useRef<number | null>(null);
  /** 每路订阅已经落进投影的最大序号：续读与实时帧重叠时据此去重。 */
  const sequencesRef = useRef<Map<string, number>>(new Map());
  /** 已经请求过的续读游标：同一个游标只续一次，避免反复重订。 */
  const resumeRef = useRef<Map<string, string>>(new Map());

  /** 续订一路被截断的订阅：先摘掉旧的那一路，再带上服务端给的游标重新订阅。 */
  const resumeSubscription = useCallback(
    (subscribeId: string, cursor: string) => {
      if (workspaceId === null) return;
      const interactionId = subscribeId.slice(INTERACTION_PREFIX.length);
      if (interactionId.length === 0) return;
      if (resumeRef.current.get(subscribeId) === cursor) return;
      resumeRef.current.set(subscribeId, cursor);
      void ipc
        .conversationUnsubscribe(subscribeId)
        .catch(() => undefined)
        .then(() => ipc.conversationWatch(workspaceId, interactionId, cursor))
        .catch(() => undefined);
    },
    [workspaceId],
  );

  const onStreamEvent = useCallback(
    (event: ConversationStreamEvent) => {
      // 一条连接承载多路订阅；只收属于自己的那几路，其余（别的视图的订阅）直接丢弃。
      if (!isSubscribed(event.subscribeId)) return;
      /*
       * 轮次头只表达「这条会话的轮次清单变了」：它不进正文投影，也不和正文帧混批。
       * 收到就唤一次索引重读，由 `appendSession` 只接本视图还不认识的那几条。
       */
      if (handleTurnHeadFrame(event)) return;
      if (event.kind === 'subscribed') {
        // 确认帧说明这次回放是否读到尾：真截断就按服务端给的真实游标续读。
        const resume = replayResume({
          replayTruncated: event.replayTruncated === true,
          resumeCursor: event.resumeCursor ?? null,
        });
        if (resume !== null) resumeSubscription(event.subscribeId, resume.resumeCursor);
      }
      if (event.kind === 'frame') {
        // 按订阅内序号去重：续读从游标之后重放，重叠帧不再重复落进投影。
        const sequence = cursorSequence(event.cursor);
        const previous = sequencesRef.current.get(event.subscribeId) ?? null;
        if (!acceptsFrameSequence(previous, sequence)) return;
        if (sequence !== null) sequencesRef.current.set(event.subscribeId, sequence);
      }
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
        // 实时增量与历史回放共用同一个归约入口：这里就是主投影唯一的帧落点。
        dispatch({ type: 'frames', frames: batch, nowMs: Date.now() });
        if (workspaceId === null) return;
        // 收到终态帧：这一轮不会再产生事件，按**帧自带的订阅标识**还回那一路订阅。
        // 不能按 `watchedRef` 退订：多轮并行时它只记得最后一条，退掉的会是另一条仍在跑的流。
        const terminalIds = new Set<string>();
        for (const frame of batch) {
          if (
            frame.kind === 'frame' &&
            (frame.eventType === 'turn_completed' || frame.eventType === 'turn_failed')
          ) {
            if (frame.subscribeId.startsWith(INTERACTION_PREFIX)) {
              terminalIds.add(frame.subscribeId.slice(INTERACTION_PREFIX.length));
            }
          }
        }
        for (const interactionId of terminalIds) {
          if (watchedRef.current === interactionId) watchedRef.current = null;
          forgetSubscription(`${INTERACTION_PREFIX}${interactionId}`);
          void ipc
            .conversationUnsubscribe(`${INTERACTION_PREFIX}${interactionId}`)
            .catch(() => undefined);
        }
      }, FRAME_FLUSH_MS);
    },
    [
      dispatch,
      forgetSubscription,
      generationRef,
      handleTurnHeadFrame,
      isSubscribed,
      resumeSubscription,
      watchedRef,
      workspaceId,
    ],
  );

  const resetFrames = useCallback(() => {
    pendingRef.current = [];
    sequencesRef.current.clear();
    resumeRef.current.clear();
  }, []);

  return { onStreamEvent, resetFrames };
}
