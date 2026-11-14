/*
 * 中断与转投的装配：车道空闲探测、摘排队消息、发出中断，三步接成一条。
 *
 * 单独成篇的原因与转投队列同源：三步的先后顺序本身就是语义（先摘消息才有得转投，
 * 等车道真的空闲才敢再提交），混在会话编排里会被别的流程切断。会话编排因此只保留
 * 一个 `interrupt` 出口。
 */

import { useCallback } from 'react';
import type { RefObject } from 'react';
import * as ipc from '../ipc';
import { useInterruptedCarry, type InterruptedCarryWiring } from './useInterruptedCarry';
import { useLaneInterrupt } from './useLaneInterrupt';

/**
 * 中断装配的依赖面：转投入参去掉 `laneIdle`（由本 hook 自己算），
 * 另加车道标识、订阅快照与提示出口。
 */
export type InterruptControlWiring = Omit<InterruptedCarryWiring, 'laneIdle'> & {
  /** 当前车道标识；会话目标下为空。 */
  laneId: string | null;
  /** 正在订阅的那一条 Interaction：车道标识缺失时用它兜底。 */
  watchedRef: RefObject<string | null>;
  /** 回执与失败原因的出口。 */
  onNotice: (message: string) => void;
};

/** 组装中断入口：Esc 与停止按钮共用这一份。 */
export function useConversationInterruptControl(
  wiring: InterruptControlWiring,
): () => Promise<void> {
  const { laneId, watchedRef, onNotice, targetKey, working, turnsRef, dispatch, submit } = wiring;
  /**
   * 车道上是否已经没有活跃轮次。
   *
   * 拿中断回执当探针：`interrupted` 为假即表示后端登记表里没有这条车道的活跃主
   * turn，此刻提交才会开出一条新的 Interaction，而不是被注入那条正在收尾的轮次。
   */
  const laneIdle = useCallback(async () => {
    const lane = laneId ?? watchedRef.current;
    if (lane === null) return true;
    try {
      const receipt = await ipc.conversationInterrupt(lane);
      return !receipt.interrupted;
    } catch {
      // 探测本身失败不拦转投：宁可把消息发出去，也不把它扣在本地。
      return true;
    }
  }, [laneId, watchedRef]);

  const handOverSteers = useInterruptedCarry({
    targetKey,
    working,
    turnsRef,
    dispatch,
    submit,
    laneIdle,
  });

  return useLaneInterrupt({
    laneId,
    watchedRef,
    onBeforeInterrupt: handOverSteers,
    onNotice,
  });
}
