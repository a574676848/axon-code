/*
 * 车道接管：让已经打开的会话视图跟上外部发起的新一轮。
 *
 * 会话事件流按 Interaction 订阅，因此后台作业跑完由 daemon 唤醒出来的那一轮、
 * 定时投递的一轮都不在订阅里：正文停在上一轮，状态位与标题却已经按车道的真实
 * 状态在走，读起来就是「活已经结束，界面还在跑」。
 *
 * 本 hook 只做编排：外部事实（车道快照上的 Interaction 总数）与内部事实（本视图
 * 已经接管到第几轮）交给 `advanceLaneSync` 判定，结论为真就要求重新接管一次。
 * 判定规则是纯函数，单测在 `tests/laneSync.test.mjs`。
 */

import { useEffect, useRef } from 'react';
import { EMPTY_LANE_SYNC_CURSOR, advanceLaneSync, type LaneSyncCursor } from './laneSync';

/**
 * 车道快照里与接管有关的事实。
 *
 * 不订阅车道的目标（子代理会话、目标执行会话）没有这份事实，传空即可。
 */
export interface LaneSyncFacts {
  /** 车道稳定标识。 */
  laneId: string;
  /** 该车道已有的 Interaction 总数；外部唤醒出来的新一轮会让它增长。 */
  interactionCount: number;
}

/** 组装参数。 */
export interface LaneSyncOptions {
  /** 当前车道的外部事实；没有可观察的车道时为空。 */
  laneFacts: LaneSyncFacts | null;
  /** 目标种类；只有车道（Interaction）目标才按轮数接管。 */
  targetKind: 'interaction' | 'session' | null;
  /** 目标是否已经完成首次加载：首屏没落地时接管会和首次加载抢同一页历史。 */
  ready: boolean;
  /** 本视图是否有正在跑的轮次。 */
  working: boolean;
  /** 重新接管：取最新一页并订阅最新那一轮。 */
  onReattach: () => void;
}

/** 按车道轮数变化接管外部发起的新一轮。 */
export function useLaneSync(options: LaneSyncOptions): void {
  const { laneFacts, targetKind, ready, working, onReattach } = options;
  const cursorRef = useRef<LaneSyncCursor>(EMPTY_LANE_SYNC_CURSOR);
  useEffect(() => {
    if (laneFacts === null || targetKind !== 'interaction' || !ready) return;
    const step = advanceLaneSync(
      cursorRef.current,
      laneFacts.laneId,
      laneFacts.interactionCount,
      working,
    );
    cursorRef.current = step.cursor;
    if (step.reattach) onReattach();
  }, [laneFacts, working, onReattach, ready, targetKind]);
}
