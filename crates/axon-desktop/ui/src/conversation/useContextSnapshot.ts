/*
 * 会话上下文快照的读取节拍。
 *
 * 三个触发点：换会话、每多一轮（轮次头刷新节拍）、以及运行期间的低频重读。
 * 重读周期与轮次头刷新同一量级，不新开高频定时器；读数带时间戳，界面自己算「更新于
 * N 秒前」，因此不需要每秒重取一次。
 *
 * 换会话时先把旧读数丢掉：宁可这一段暂时不占位，也不让上一条会话的占用挂在新会话上。
 */

import { useEffect, useMemo, useState } from 'react';

import { contextSnapshotReading, type ContextSnapshot, type ContextSnapshotReading } from './contextSnapshot';
import { conversationContextSnapshot } from './sessionEndpoints';

/** 运行期间的重读周期。 */
export const SNAPSHOT_REFRESH_MS = 15_000;

/**
 * 「更新于 N 秒前」的刷新粒度。
 *
 * 相对时间只能按这一档重新计算：每帧重算会让读数对象每帧换引用，正文里消费它的
 * 每一轮都跟着重建。10 秒足够让「刚刚」平滑过渡到「12 秒前」，也省掉一个高频定时器。
 */
const AGE_BUCKET_MS = 10_000;

/** 一份快照与它的所属会话：跨会话的读数一律不采用。 */
interface SnapshotState {
  sessionId: string;
  snapshot: ContextSnapshot | null;
}

/**
 * 读当前会话的上下文快照，并换算成界面读数。
 *
 * `revision` 变化即重读一次：调用方传轮次数量，新的一轮开始或收口都会触发。
 * `configuredWindow` 是当前生效模型在配置目录里的窗口，作为分母的回退值：用量帧通常
 * 不带窗口字段，此时用它是「这一轮真正按它算的预算」，不是拿旧读数顶替。
 * 返回 null 表示还没有任何可用读数，此时头部上下文段整段不占位。
 */
export function useContextSnapshot(
  sessionId: string | null,
  revision: number,
  configuredWindow: number | null = null,
): ContextSnapshotReading | null {
  const [state, setState] = useState<SnapshotState | null>(null);

  useEffect(() => {
    if (sessionId === null) return undefined;
    let cancelled = false;
    const load = () => {
      void conversationContextSnapshot(sessionId)
        .then((reading) => {
          if (cancelled) return;
          setState({ sessionId, snapshot: reading.snapshot });
        })
        .catch(() => {
          // 读失败保留上一份读数：时间戳没变，界面自己会显示它有多旧。
        });
    };
    load();
    const timer = window.setInterval(load, SNAPSHOT_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [sessionId, revision]);

  const current = sessionId !== null && state?.sessionId === sessionId ? state.snapshot : null;
  const ageBucket = Math.floor(Date.now() / AGE_BUCKET_MS) * AGE_BUCKET_MS;
  return useMemo(
    () => contextSnapshotReading(current, ageBucket, configuredWindow),
    [current, ageBucket, configuredWindow],
  );
}
