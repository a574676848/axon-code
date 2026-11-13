/*
 * 车道中断：Esc 与停止按钮的唯一实现。
 *
 * 中断之前先把还没被注入的排队消息摘下来交给转投队列：后端只在安全边界把它们插入本轮，
 * 取消之后它们再也等不到那次边界，内容就随本轮一起消失了。
 *
 * 一次按键只发一次中断：Esc 长按会连发 keydown，而每次中断都是一次真实的后端请求，
 * 在途期间的重复请求只会换来「没有正在运行的一轮」这种噪声结论。
 */

import { useCallback, useRef } from 'react';
import * as ipc from '../ipc';

export interface LaneInterruptOptions {
  /** 当前车道标识；会话目标下为空。 */
  laneId: string | null;
  /** 正在订阅的那一条 Interaction：车道标识缺失时用它兜底。 */
  watchedRef: React.RefObject<string | null>;
  /** 真正发请求之前的动作（摘排队消息转投）。 */
  onBeforeInterrupt: () => void;
  /** 回执与失败原因的出口。 */
  onNotice: (message: string) => void;
}

/** 中断当前车道正在运行的一轮；回执为假时如实提示，不假装成功。 */
export function useLaneInterrupt(options: LaneInterruptOptions): () => Promise<void> {
  const { laneId, watchedRef, onBeforeInterrupt, onNotice } = options;
  /** 中断请求是否在途。 */
  const interruptingRef = useRef(false);

  return useCallback(async () => {
    const lane = laneId ?? watchedRef.current;
    if (lane === null) return;
    if (interruptingRef.current) return;
    interruptingRef.current = true;
    onBeforeInterrupt();
    try {
      const receipt = await ipc.conversationInterrupt(lane);
      if (!receipt.interrupted) {
        onNotice('本轮已经结束，无需中断');
      }
    } catch (error) {
      onNotice(error instanceof Error ? error.message : '中断失败');
    } finally {
      interruptingRef.current = false;
    }
  }, [laneId, onBeforeInterrupt, onNotice, watchedRef]);
}
