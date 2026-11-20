/*
 * 轮次头订阅的编排：把「这条会话的轮次清单变了」翻译成一次索引重读。
 *
 * 从会话编排里整块搬出：这里只处理三个定时器与一个订阅，和正文投影没有任何关系；
 * 留在编排文件里既把它推过行数硬限，也让「什么时候该重读索引」这条规则埋在流程中间。
 *
 * 三条规则：
 * - 认识的轮次不重读（正文与收口由正文订阅推进），只有外部新轮才读一次；
 * - 合并窗口固定，突发帧只产生一次重读；
 * - 订阅断开先自己续，超过上限才把事实交给界面。
 */

import { useCallback, useRef } from 'react';

import * as ipc from '../ipc';
import type { ConversationStreamEvent } from './wire';
import { handleTurnsFrame, SESSION_TURNS_PREFIX } from './sessionTurns';
import type { ConversationAction } from './transcript';
import type { ConversationTarget } from './targets';

/** 轮次头帧的合并窗口：一轮刚创建时可能连发数帧，只唤一次索引。 */
const TURNS_REFRESH_COALESCE_MS = 250;
/** 订阅断开后的自动重试间隔与次数上限。 */
const TURNS_RETRY_MS = 2_000;
const TURNS_RETRY_LIMIT = 3;

export interface TurnHeadRefreshOptions {
  /** 当前目标：重试时要用它的工作区与会话标识重新订阅。 */
  targetRef: React.MutableRefObject<ConversationTarget | null>;
  /** 本视图已经持有的轮次标识。 */
  knownTurnIds: () => readonly string[];
  /** 重读一次索引：由会话分页层按标识去重，因此重复调用不会重复接轮。 */
  appendSession: () => void;
  /** 失败帧的落点：与正文共用同一个归约入口。 */
  dispatch: (action: ConversationAction) => void;
}

export interface TurnHeadRefresh {
  /** 处理一条帧；返回 `true` 表示已被轮次头消费。 */
  handle: (event: ConversationStreamEvent) => boolean;
  /** 换目标时复位重试预算。 */
  reset: () => void;
  /** 清理定时器：换目标、卸载与重连都要做。 */
  clearTimers: () => void;
}

export function useTurnHeadRefresh(options: TurnHeadRefreshOptions): TurnHeadRefresh {
  const { targetRef, knownTurnIds, appendSession, dispatch } = options;
  // 回调每次渲染都可能换身份；真正要用它们的是定时器，因此走 ref，保持本 hook 稳定。
  const appendRef = useRef(appendSession);
  appendRef.current = appendSession;
  const knownRef = useRef(knownTurnIds);
  knownRef.current = knownTurnIds;

  const refreshTimerRef = useRef<number | null>(null);
  const requestRefresh = useCallback(() => {
    if (refreshTimerRef.current !== null) return;
    refreshTimerRef.current = window.setTimeout(() => {
      refreshTimerRef.current = null;
      appendRef.current();
    }, TURNS_REFRESH_COALESCE_MS);
  }, []);

  const retryCountRef = useRef(0);
  const retryTimerRef = useRef<number | null>(null);
  const scheduleRetry = useCallback(() => {
    if (retryTimerRef.current !== null) return;
    if (retryCountRef.current >= TURNS_RETRY_LIMIT) {
      dispatch({
        type: 'frames',
        frames: [
          {
            kind: 'failure',
            subscribeId: `${SESSION_TURNS_PREFIX}${targetRef.current?.interactionId ?? ''}`,
            message: '新轮次订阅已断开，界面不会自动发现新轮次',
            retryable: true,
          },
        ],
        nowMs: Date.now(),
      });
      return;
    }
    retryCountRef.current += 1;
    retryTimerRef.current = window.setTimeout(() => {
      retryTimerRef.current = null;
      const current = targetRef.current;
      if (current === null) return;
      void ipc
        .conversationWatchTurns(current.workspaceId, current.interactionId)
        .then(() => {
          // 重订阅成功即归还预算：否则一次会话里累计断三次之后，后续断流只报「界面不会
          // 自动发现新轮次」，外部发起的新轮次再也不接管。
          retryCountRef.current = 0;
        })
        .catch(() => undefined);
    }, TURNS_RETRY_MS);
  }, [dispatch, targetRef]);

  const handle = useCallback(
    (event: ConversationStreamEvent) =>
      handleTurnsFrame(event, knownRef.current(), {
        requestRefresh,
        onFailure: scheduleRetry,
      }),
    [requestRefresh, scheduleRetry],
  );

  const reset = useCallback(() => {
    retryCountRef.current = 0;
  }, []);

  const clearTimers = useCallback(() => {
    if (refreshTimerRef.current !== null) {
      window.clearTimeout(refreshTimerRef.current);
      refreshTimerRef.current = null;
    }
    if (retryTimerRef.current !== null) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
  }, []);

  return { handle, reset, clearTimers };
}
