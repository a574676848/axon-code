/*
 * 车道接管：让已经打开的会话视图跟上外部发起的新一轮。
 *
 * 会话事件流按 Interaction 订阅，因此后台作业跑完由 daemon 唤醒出来的那一轮、
 * 定时投递的一轮都不在订阅里：正文停在上一轮，状态位与标题却已经按车道的真实
 * 状态在走，读起来就是「活已经结束，界面还在跑」。
 *
 * 本 hook 只做编排：外部事实（车道快照上的 Interaction 总数）与内部事实（本视图
 * 已经接管到第几轮）交给 `advanceSessionSync` 判定，结论为真就要求重新接管一次。
 * 判定规则是纯函数，单测在 `tests/sessionSync.test.mjs`。
 */

import { useEffect, useRef } from 'react';
import { EMPTY_SESSION_SYNC_CURSOR, advanceSessionSync, type SessionSyncCursor } from './sessionSync';

/**
 * 车道快照里与接管有关的事实。
 *
 * 不订阅车道的目标（子代理会话、目标执行会话）没有这份事实，传空即可。
 */
export interface SessionSyncFacts {
  /** 车道稳定标识。 */
  sessionId: string;
  /** 该车道已有的 Interaction 总数；外部唤醒出来的新一轮会让它增长。 */
  interactionCount: number;
}

/** 组装参数。 */
export interface SessionSyncOptions {
  /** 当前会话的外部事实；这条会话不在会话清单里时为空（例如目标执行会话）。 */
  sessionFacts: SessionSyncFacts | null;
  /** 目标是否已经完成首次加载：首屏没落地时接管会和首次加载抢同一页历史。 */
  ready: boolean;
  /** 本视图是否有正在跑的轮次。 */
  working: boolean;
  /** 把外部发起的新一轮接进投影：只接新增轮次，不重置、不重放已有历史。 */
  onReattach: () => void;
}

/** 按会话轮数变化接管外部发起的新一轮。 */
export function useSessionSync(options: SessionSyncOptions): void {
  const { sessionFacts, ready, working, onReattach } = options;
  const cursorRef = useRef<SessionSyncCursor>(EMPTY_SESSION_SYNC_CURSOR);
  useEffect(() => {
    if (sessionFacts === null || !ready) return;
    const step = advanceSessionSync(
      cursorRef.current,
      sessionFacts.sessionId,
      sessionFacts.interactionCount,
      working,
    );
    cursorRef.current = step.cursor;
    if (step.reattach) onReattach();
  }, [sessionFacts, working, onReattach, ready]);
}
