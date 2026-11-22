import React from 'react';

import type { ContextSnapshotReading } from '../../conversation/contextSnapshot';

/*
 * 会话上下文快照的注入点。
 *
 * 头部与正文里的换窗进度行读的是同一份读数：两处各取一次快照就会出现「头部说 317K、
 * 换窗行说 825K」这种同屏两套数字。因此快照只在视图层取一次，往下按上下文注入，
 * 任何消费者拿到的都是同一个对象、同一个时间戳。
 */

/** 当前会话的上下文读数；没有快照或还没读回来时为空。 */
const ContextSnapshotContext = React.createContext<ContextSnapshotReading | null>(null);

export const ContextSnapshotProvider: React.FC<{
  /** 视图层算好的读数；换会话时一起换掉。 */
  reading: ContextSnapshotReading | null;
  children: React.ReactNode;
}> = ({ reading, children }) => (
  <ContextSnapshotContext.Provider value={reading}>{children}</ContextSnapshotContext.Provider>
);

/** 读当前会话的上下文读数。 */
export function useContextSnapshotReading(): ContextSnapshotReading | null {
  return React.useContext(ContextSnapshotContext);
}
