import React from 'react';
import * as ipc from './ipc';
import type {
  ConnectionSnapshot,
  ConnectionTestInput,
  ConnectionTestResult,
  SwitchOutcome,
} from './connectionTypes';

/*
 * 连接读数的事实源。
 *
 * 一个事实源 + 明确的唯一入口 + 其余落点只引用不重取：侧栏连接胶囊与设置 › 连接
 * 读的是同一份快照，因此不会出现「胶囊说已连接、分区说未连接」。任何落点都不另外
 * 发起探测，也不在本地推导健康态。
 *
 * 探活不是轮询：只在启动首帧之后、用户手动重试、切换面板展开时各发一次，不设固定
 * 定时器。状态存在模块里，组件用 useSyncExternalStore 订阅，不另建一层上下文。
 */

/** 连接事实。 */
export interface ConnectionFact {
  /** 最近一次快照；尚未取到时为空。 */
  snapshot: ConnectionSnapshot | null;
  /** 是否正在取快照或探测。 */
  busy: boolean;
  /** 命令本身没跑通的原因；与「连接结论是连不上」是两回事。 */
  error: string | null;
}

const INITIAL: ConnectionFact = { snapshot: null, busy: false, error: null };

let state: ConnectionFact = INITIAL;
const listeners = new Set<() => void>();
/** 正在跑的那一次读取；同一时刻只允许一份。 */
let inFlight: Promise<void> | null = null;
/** 是否已经自动读过一次：进程内只自动读一次，重试由用户显式触发。 */
let autoStarted = false;

function publish(next: ConnectionFact): void {
  state = next;
  for (const listener of listeners) listener();
}

/** 订阅连接事实，供 useSyncExternalStore 使用。 */
export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 当前快照。引用只在事实变化时换，因此订阅方不会白渲染。 */
export function snapshot(): ConnectionFact {
  return state;
}

/** 读取一次快照。 */
export function loadConnectionState(): Promise<void> {
  if (inFlight !== null) return inFlight;
  publish({ ...state, busy: true, error: null });
  inFlight = ipc
    .connectionState()
    .then((next) => {
      publish({ ...state, snapshot: next, busy: false, error: null });
    })
    .catch((failure: unknown) => {
      publish({
        ...state,
        busy: false,
        error: failure instanceof Error ? failure.message : '读取连接状态失败',
      });
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

/** 重新探测当前生效档案。 */
export function retryProbe(): Promise<void> {
  publish({ ...state, busy: true, error: null });
  return ipc
    .connectionRetryProbe()
    .then((next) => {
      publish({ ...state, snapshot: next, busy: false, error: null });
    })
    .catch((failure: unknown) => {
      publish({
        ...state,
        busy: false,
        error: failure instanceof Error ? failure.message : '重试连接失败',
      });
    });
}

/**
 * 延迟读数的唯一口径：取到写「N 毫秒」，没取到返回 null 由调用方决定怎么呈现。
 *
 * 未取到能力位、尚未探测与连不上都属「没有读数」，绝不写 0 或伪造值。
 */
export function latencyText(latencyMs: number | null): string | null {
  return latencyMs === null ? null : `${latencyMs} 毫秒`;
}

/** 试连接。 */
export function testConnection(input: ConnectionTestInput): Promise<ConnectionTestResult> {
  return ipc.connectionTest(input);
}

/** 保存云档案并刷新快照。 */
export function saveProfile(label: string, address: string): Promise<void> {
  return ipc.connectionSaveProfile(label, address).then((next) => {
    publish({ ...state, snapshot: next });
  });
}

/** 删除档案并刷新快照。 */
export function deleteProfile(id: string): Promise<void> {
  return ipc.connectionDeleteProfile(id).then((next) => {
    publish({ ...state, snapshot: next });
  });
}

/** 重命名档案并刷新快照。 */
export function renameProfile(id: string, label: string): Promise<void> {
  return ipc.connectionRenameProfile(id, label).then((next) => {
    publish({ ...state, snapshot: next });
  });
}

/**
 * 切档前的确认：由外壳注册，返回 false 表示用户放弃切换。
 *
 * 切档以重启应用实现，输入坞里没发出去的内容会丢。三个入口（侧栏胶囊、设置 › 连接、
 * 命令面板）都走这里，因此确认只需要一处实现，不会出现「从某个入口切就不问」。
 */
let switchGuard: (() => boolean | Promise<boolean>) | null = null;

/** 注册或清除切档确认。 */
export function registerSwitchGuard(guard: (() => boolean | Promise<boolean>) | null): void {
  switchGuard = guard;
}

/** 切换档案。 */
export async function switchProfile(id: string): Promise<SwitchOutcome> {
  if (switchGuard !== null && !(await switchGuard())) {
    return { started: false, message: '已取消切换：输入框里还有未发送的内容' };
  }
  return ipc.connectionSwitchProfile(id);
}

/** 记录本次会话内输入的凭据。 */
export function setSessionSecret(secret: string): Promise<void> {
  return ipc.connectionSetSessionSecret(secret).then((next) => {
    publish({ ...state, snapshot: next });
  });
}

/** 记录引导完成并刷新快照。 */
export function completeOnboarding(): Promise<void> {
  return ipc.connectionCompleteOnboarding().then((next) => {
    publish({ ...state, snapshot: next });
  });
}

/** 跳过引导并刷新快照。 */
export function skipOnboarding(): Promise<void> {
  return ipc.connectionSkipOnboarding().then((next) => {
    publish({ ...state, snapshot: next });
  });
}

/**
 * 启动时自动读一次；重复调用不会重复请求。
 *
 * 失败不在本地重试：自动重试会在离线的机器上反复打同一个端点，把一次瞬时故障变成
 * 一串请求；要不要再试由用户点「重试连接」决定。
 */
export function ensureConnectionLoaded(): void {
  if (autoStarted) return;
  autoStarted = true;
  void loadConnectionState();
}

/** 订阅连接事实的 React 接口。 */
export function useConnectionFact(): ConnectionFact {
  return React.useSyncExternalStore(subscribe, snapshot, snapshot);
}
