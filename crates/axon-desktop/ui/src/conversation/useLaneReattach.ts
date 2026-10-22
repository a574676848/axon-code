import { useCallback, useState } from 'react';

/*
 * 重新贴住车道的请求序号。
 *
 * 车道装配以目标为键：贴一次就是「重置投影、重取历史索引、逐条回放」。问答作答走
 * 消息通道时，续跑的那一轮由守护进程开启，界面在此之前根本不知道有这条 Interaction，
 * 因此只能靠重贴把它拉回视图；回灌给活等待者时不必重贴，那条订阅还在看着同一轮。
 */

export interface LaneReattach {
  /** 请求序号：作为车道装配的键参与依赖，变一次就重贴一次。 */
  token: number;
  /** 请求重贴一次。 */
  reattach: () => void;
}

/** 取「重新贴住车道」的请求序号与触发函数。 */
export function useLaneReattach(): LaneReattach {
  const [token, setToken] = useState(0);
  const reattach = useCallback(() => setToken((current) => current + 1), []);
  return { token, reattach };
}
