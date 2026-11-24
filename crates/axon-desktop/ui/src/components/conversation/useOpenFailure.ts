import React from 'react';

import { nextOpenFailure } from '../../conversation/openFailure';

/*
 * 打开会话失败的持有者。
 *
 * 一句话说明为什么要有这一层：失败回执在控制器里活 6 秒，而「这条会话读不回来」
 * 是一个需要一直摆在屏幕上、并且带重试入口的结论。因此这里把回执抄一份留在这里，
 * 只在两种情况下撤下：正文终于落地，或用户点了重连。
 */

export interface OpenFailureOptions {
  /** 控制器当前的失败回执。 */
  error: string | null;
  /** 正文还在装载。 */
  loadingHistory: boolean;
  /** 过渡窗口还没关闭。 */
  opening: boolean;
  /** 投影里是否已经有正文。 */
  hasTurns: boolean;
  /** 当前会话键：换会话即换一份结论。 */
  sessionKey: string | null;
}

export interface OpenFailureState {
  /** 需要渲染错误卡的原因；为空表示没有。 */
  openFailure: string | null;
  /** 重连：先撤下这张卡，再让控制器重开一次订阅与回放。 */
  retry: () => void;
}

export function useOpenFailure({
  error,
  loadingHistory,
  opening,
  hasTurns,
  sessionKey,
}: OpenFailureOptions, onRetry: () => void): OpenFailureState {
  const [openFailure, setOpenFailure] = React.useState<string | null>(null);
  /** 结论属于哪条会话：换一条即作废，不把上一条的失败挂到新会话上。 */
  const [failureKey, setFailureKey] = React.useState<string | null>(sessionKey);
  if (failureKey !== sessionKey) {
    setFailureKey(sessionKey);
    setOpenFailure(null);
  }

  React.useEffect(() => {
    setOpenFailure((previous) =>
      nextOpenFailure(previous, { error, loadingHistory, opening, hasTurns }),
    );
  }, [error, loadingHistory, opening, hasTurns]);

  const retry = React.useCallback(() => {
    setOpenFailure(null);
    onRetry();
  }, [onRetry]);

  return { openFailure, retry };
}
