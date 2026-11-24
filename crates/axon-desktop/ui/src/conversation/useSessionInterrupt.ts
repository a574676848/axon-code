/*
 * 车道中断：Esc 与停止按钮的唯一实现。
 *
 * 中断之前先把还没被注入的排队消息摘下来交给转投队列：后端只在安全边界把它们插入本轮，
 * 取消之后它们再也等不到那次边界，内容就随本轮一起消失了。
 *
 * 一次按键只发一次中断：Esc 长按会连发 keydown，而每次中断都是一次真实的后端请求，
 * 在途期间的重复请求只会换来「没有正在运行的一轮」这种噪声结论。
 *
 * 回执为假时不能直接宣布「已经结束」：索引可能仍说这一轮在途，那正是界面转圈不消失的
 * 现场。结论与文案由 `interruptOutcome` 按「回执 + 重读一次的索引」给出（见该模块）。
 */

import { useCallback, useRef } from 'react';
import * as ipc from '../ipc';
import { interruptOutcome, readLatestTurnReading } from './interruptOutcome';
import { conversationSettleSilentTurn } from './sessionEndpoints';

export interface SessionInterruptOptions {
  /** 当前车道标识；会话目标下为空。 */
  sessionId: string | null;
  /** 正在订阅的那一条 Interaction：车道标识缺失时用它兜底。 */
  watchedRef: React.RefObject<string | null>;
  /** 真正发请求之前的动作（摘排队消息转投）。 */
  onBeforeInterrupt: () => void;
  /** 回执与失败原因的出口。 */
  onNotice: (message: string) => void;
}

/** 中断当前车道正在运行的一轮；回执为假时按索引复核结论，不假装成功。 */
export function useSessionInterrupt(options: SessionInterruptOptions): () => Promise<void> {
  const { sessionId, watchedRef, onBeforeInterrupt, onNotice } = options;
  /** 中断请求是否在途。 */
  const interruptingRef = useRef(false);

  return useCallback(async () => {
    const session = sessionId ?? watchedRef.current;
    if (session === null) return;
    if (interruptingRef.current) return;
    interruptingRef.current = true;
    onBeforeInterrupt();
    try {
      const receipt = await ipc.conversationInterrupt(session);
      const outcome = await interruptOutcome(
        receipt,
        () =>
          readLatestTurnReading(async () => {
            // 只取最新一条：结论只依赖这一轮现在在不在跑，以及它是不是要在服务端收口。
            const page = await ipc.conversationHistory(session, 1);
            return page.interactions;
          }),
        /*
         * 索引仍说在途时请服务端收口：界面不自己改状态，只转达服务端的判据。
         * 旧后台没有这条路由时返回 null，界面如实说「没有可用的收口接口」。
         */
        (interactionId) =>
          conversationSettleSilentTurn(session, interactionId).catch(() => null),
      );
      if (outcome.notice !== null) onNotice(outcome.notice);
    } catch (error) {
      onNotice(error instanceof Error ? error.message : '中断失败');
    } finally {
      interruptingRef.current = false;
    }
  }, [sessionId, onBeforeInterrupt, onNotice, watchedRef]);
}
