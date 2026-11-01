import React, { useCallback, useEffect, useRef, useState } from 'react';
import * as ipc from './ipc';
import { retryProbe } from './connectionState';
import type { GatewayHealth } from './types';

/*
 * 连接探活：全应用只有这一处发探测。
 *
 * 两个事实分开保管但同源触发：
 * - `health`：网关可用性，决定主区域是正常界面还是「未连接到 Axon 后台服务」；
 * - 连接档案读数（connectionState）：探的是 `/api/v1/meta`，决定胶囊与能力位。
 *
 * 触发时机只有四种：启动首帧之后、后端调用报不可用、窗口从隐藏恢复可见、用户点重试。
 * 不设固定定时器：合同 §8.7 明确禁止轮询式探活。
 */

/** 探活与断连弹窗共享的连接事实。 */
export interface ConnectionProbe {
  /** 网关可用性；尚未探测时为 null。 */
  health: GatewayHealth | null;
  /** 最近一次网关调用失败的原因，供断连页展示。 */
  lastFailure: string | null;
  /** 断连弹窗是否已被用户关掉。 */
  connectionDismissed: boolean;
  /** 关掉或重新打开断连弹窗。 */
  setConnectionDismissed: (value: boolean) => void;
  /** 当前是否已连上，供不依赖渲染时机的回调读取。 */
  connectionRef: React.MutableRefObject<boolean>;
  /** 只探网关可用性。 */
  probeGateway: () => Promise<void>;
  /** 网关可用性与连接档案读数一起探。 */
  refreshConnection: () => Promise<void>;
}

/** 界面存活报到间隔。 */
const HEARTBEAT_MS = 5000;

/** 装配连接探活。 */
export function useConnectionProbe(): ConnectionProbe {
  const [health, setHealth] = useState<GatewayHealth | null>(null);
  const [lastFailure, setLastFailure] = useState<string | null>(null);
  const [connectionDismissed, setConnectionDismissed] = useState(false);
  const connectionRef = useRef(false);

  /** 上一次探测是否仍在途：单次探测可长达数秒，不能叠加第二次。 */
  const probingRef = useRef(false);

  const probeGateway = useCallback(async () => {
    probingRef.current = true;
    try {
      const result = await ipc.gatewayHealth();
      if (result.connected || connectionRef.current === true) setConnectionDismissed(false);
      // 连上了就没有「最近一次失败」可讲：留着它只会让下一次断连的弹窗显示旧原因。
      if (result.connected) setLastFailure(null);
      connectionRef.current = result.connected;
      setHealth(result);
    } catch {
      // 探测本身失败（例如桌面宿主不可用）时保持上一次结论，由界面上的重试接管。
    } finally {
      probingRef.current = false;
    }
  }, []);

  const refreshConnection = useCallback(async () => {
    await probeGateway();
    await retryProbe();
  }, [probeGateway]);

  useEffect(() => {
    // 先出界面，再探后端：首帧不以任何网络探测为前置。
    void probeGateway();
    void retryProbe();
  }, [probeGateway]);

  // 窗口从隐藏恢复可见时重新探一次：隐藏期间不跑定时器，回来后要一次新结论。
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState !== 'visible') return;
      void refreshConnection();
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => document.removeEventListener('visibilitychange', onVisible);
  }, [refreshConnection]);

  /*
   * 网络请求拦截器：任何一个后端调用报「服务未启用」，都立即复探。
   *
   * 不依赖各视图各自解释同一个断连原因：用户看到的第一个失败就已经把结论带到这里。
   * 用户此前的手动关闭会被重新打开：服务又断了一次是新的信息，不该被上一次的
   * 「知道了」永久压住。
   */
  useEffect(
    () =>
      ipc.onBackendUnavailable((detail) => {
        setLastFailure(detail);
        setConnectionDismissed(false);
        void refreshConnection();
      }),
    [refreshConnection],
  );

  /*
   * 存活报到：每 5 秒一条轻量 IPC，让宿主知道界面还活着。
   *
   * 它与探活是两件事：探活问「后端能用吗」，这条只报「界面还在」。看门狗在 20 秒
   * 没收到报到时会把窗口切到兜底页，所以这条心跳必须常驻；探活则按事件触发，
   * 不设定时器。
   */
  useEffect(() => {
    const beat = () => {
      void ipc.uiHeartbeat().catch(() => undefined);
    };
    beat();
    const timer = window.setInterval(beat, HEARTBEAT_MS);
    return () => window.clearInterval(timer);
  }, []);

  return {
    health,
    lastFailure,
    connectionDismissed,
    setConnectionDismissed,
    connectionRef,
    probeGateway,
    refreshConnection,
  };
}
