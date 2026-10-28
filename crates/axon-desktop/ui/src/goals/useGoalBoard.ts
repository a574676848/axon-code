/*
 * 目标看板的数据侧：一次读取、静默轮询与详情刷新。
 *
 * 四条纪律（对应性能与安全合同）：
 * 1. 只在存在未收口目标时每 4 秒静默刷新，全部收口即停表；
 * 2. 请求串行化并带序号，旧响应不得覆盖新响应；
 * 3. 隐藏窗口的 tick 直接跳过本轮，不新增任何事件监听，恢复可见后下一拍自愈；
 * 4. 轮询失败不弹窗：连续失败才由页面错误条说明，下一次成功自愈即消失。
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import * as ipc from '../goalIpc';
import type { GoalDetailView, GoalListView } from '../goalTypes';
import { hasUnsettled, isSettled } from './model';

/** 静默刷新间隔。 */
export const GOAL_POLL_INTERVAL_MS = 4_000;
/** 连续失败达到这个次数才把失败摆到页面上，避免一次抖动就打断阅读。 */
export const POLL_FAILURE_THRESHOLD = 2;

/** 看板数据侧对外暴露的状态。 */
export interface GoalBoardFeed {
  view: GoalListView | null;
  /** 首屏加载中。 */
  loading: boolean;
  /** 首屏读取失败；有数据之后不再使用，改由 `pollError` 说明。 */
  error: string | null;
  /** 连续轮询失败。 */
  pollError: string | null;
  /** 手动刷新；与轮询共用同一条串行通道。 */
  refresh: () => void;
}

interface LoadOptions {
  /** 隐藏窗口跳过本轮；只有轮询会跳过，首屏与手动刷新不跳过。 */
  skipWhenHidden: boolean;
}

/** 失败原因归一：IPC 抛出的可能是 Error、字符串或任意外部值。 */
function failureMessage(failure: unknown): string {
  if (failure instanceof Error) return failure.message;
  if (typeof failure === 'string' && failure.length > 0) return failure;
  return '读取目标看板失败';
}

/** 隐藏窗口判定；无头环境按可见处理。 */
function isHidden(): boolean {
  return typeof document !== 'undefined' && document.visibilityState === 'hidden';
}

export function useGoalBoard(paused = false): GoalBoardFeed {
  const [view, setView] = useState<GoalListView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pollError, setPollError] = useState<string | null>(null);
  /** 请求序号：只有最后一次发出的响应可以落地。 */
  const sequenceRef = useRef(0);
  /** 串行闸门：前一次未回来时不发第二次。 */
  const inFlightRef = useRef(false);
  /** 连续失败次数。 */
  const failuresRef = useRef(0);
  /** 是否已经有过一次成功读取；决定失败落到「首屏错误」还是「轮询错误」。 */
  const hasDataRef = useRef(false);
  const mountedRef = useRef(true);

  const load = useCallback(async (options: LoadOptions) => {
    if (options.skipWhenHidden && isHidden()) return;
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    const sequence = sequenceRef.current + 1;
    sequenceRef.current = sequence;
    try {
      const next = await ipc.goalList();
      if (!mountedRef.current || sequence !== sequenceRef.current) return;
      failuresRef.current = 0;
      hasDataRef.current = true;
      setPollError(null);
      setError(null);
      setView(next);
    } catch (failure) {
      if (!mountedRef.current || sequence !== sequenceRef.current) return;
      failuresRef.current += 1;
      const message = failureMessage(failure);
      if (!hasDataRef.current) {
        // 还没有任何数据可看：这是首屏错误，直接摆在页面上并给重试。
        setError(message);
      } else if (failuresRef.current >= POLL_FAILURE_THRESHOLD) {
        // 有数据之后只在连续失败时才提示，一次抖动不打断阅读。
        setPollError(message);
      }
    } finally {
      inFlightRef.current = false;
      if (mountedRef.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    void load({ skipWhenHidden: false });
    return () => {
      mountedRef.current = false;
    };
  }, [load]);

  const unsettled = view !== null && hasUnsettled(view.goals);

  useEffect(() => {
    if (!unsettled || paused) return undefined;
    const timer = window.setInterval(
      () => void load({ skipWhenHidden: true }),
      GOAL_POLL_INTERVAL_MS,
    );
    return () => window.clearInterval(timer);
  }, [unsettled, paused, load]);

  const refresh = useCallback(() => {
    void load({ skipWhenHidden: false });
  }, [load]);

  return { view, loading, error, pollError, refresh };
}

/** 详情数据侧的状态。 */
export interface GoalDetailFeed {
  detail: GoalDetailView | null;
  loading: boolean;
  error: string | null;
}

/**
 * 单个目标的详情。
 *
 * `active` 为真时才拉取，并且只在目标未收口时继续刷新：收口的目标不会再有新事实。
 * 与看板同一套纪律：请求串行化并带序号，旧响应不得覆盖新响应；隐藏窗口跳过本轮；
 * 目标一换就先清空上一份详情，避免弹窗里短暂显示上一个目标的步骤与验收。
 */
export function useGoalDetail(goalId: string | null, active: boolean): GoalDetailFeed {
  const [detail, setDetail] = useState<GoalDetailView | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  /** 请求序号：只有最后一次发出的响应可以落地。 */
  const sequenceRef = useRef(0);
  /** 串行闸门：前一次未回来时不发第二次。 */
  const inFlightRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const load = useCallback(async (id: string, skipWhenHidden: boolean) => {
    if (skipWhenHidden && isHidden()) return;
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    const sequence = sequenceRef.current + 1;
    sequenceRef.current = sequence;
    try {
      const next = await ipc.goalGet(id);
      if (!mountedRef.current || sequence !== sequenceRef.current) return;
      setDetail(next);
      setError(null);
    } catch (failure) {
      if (!mountedRef.current || sequence !== sequenceRef.current) return;
      setError(failureMessage(failure));
    } finally {
      inFlightRef.current = false;
      if (mountedRef.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!active || goalId === null) return undefined;
    // 换目标先清空：旧目标的步骤与验收不能在新目标的面板上停留一拍。
    setDetail(null);
    setError(null);
    setLoading(true);
    void load(goalId, false);
    return undefined;
  }, [active, goalId, load]);

  const unsettled = detail !== null && !isSettled(detail.goal.status);

  useEffect(() => {
    if (!active || goalId === null || !unsettled) return undefined;
    const timer = window.setInterval(
      () => void load(goalId, true),
      GOAL_POLL_INTERVAL_MS,
    );
    return () => window.clearInterval(timer);
  }, [active, goalId, unsettled, load]);

  return { detail, loading, error };
}
