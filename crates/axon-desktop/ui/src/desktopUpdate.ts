import React from 'react';
import * as ipc from './ipc';
import type { UpdateCheck } from './types';

/*
 * 桌面端的版本更新事实。
 *
 * 检测结论只有一份：应用启动时自动检测一次，设置 › 关于与侧栏底部的入口都读这一份，
 * 因此两处不会各发一次请求，也不会出现「关于页说有新版本、侧栏却说没有」这种分叉。
 *
 * 状态存在模块里，组件用 useSyncExternalStore 订阅，不另建一层上下文。
 */

/** 检测阶段：还没开始、正在检测、已拿到结论（结论本身可能是失败）。 */
export type UpdatePhase = 'idle' | 'checking' | 'ready';

/** 一份更新事实。 */
export interface UpdateState {
  phase: UpdatePhase;
  /** 后端回执；还没检测完为空。 */
  result: UpdateCheck | null;
  /** 命令本身没跑通的原因；与「检测结论是失败」是两回事。 */
  error: string | null;
}

const INITIAL: UpdateState = { phase: 'idle', result: null, error: null };

let state: UpdateState = INITIAL;
const listeners = new Set<() => void>();
/** 正在跑的那一次检测；同一时刻只允许一份，按钮与自动检测共用它。 */
let inFlight: Promise<void> | null = null;
/** 自动检测是否已经发起过：进程内只自动跑一次，重试由用户显式触发。 */
let autoStarted = false;

function publish(next: UpdateState): void {
  state = next;
  for (const listener of listeners) listener();
}

/** 订阅更新事实，供 useSyncExternalStore 使用。 */
export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 当前快照。引用只在事实变化时换，因此订阅方不会白渲染。 */
export function snapshot(): UpdateState {
  return state;
}

/**
 * 检测一次更新。
 *
 * 重检期间保留上一份结论：关于页的读数因此不会先塌成「尚未检测」再长回来，
 * 侧栏入口也不会在检测过程中闪一下。
 */
export function checkForUpdate(): Promise<void> {
  if (inFlight !== null) return inFlight;
  publish({ phase: 'checking', result: state.result, error: null });
  inFlight = ipc
    .checkUpdate()
    .then((result) => {
      publish({ phase: 'ready', result, error: null });
    })
    .catch((failure: unknown) => {
      publish({
        phase: 'ready',
        result: null,
        error: failure instanceof Error ? failure.message : '检测更新失败',
      });
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

/**
 * 启动时自动检测一次；重复调用不会重复请求。
 *
 * 失败不在这里重试：自动重试会在离线的机器上反复打同一个端点，把一次瞬时故障
 * 变成一串请求；要不要再试由用户点「检测更新」或侧栏入口决定。
 */
export function ensureUpdateChecked(): void {
  if (autoStarted) return;
  autoStarted = true;
  void checkForUpdate();
}

/** 订阅更新事实的 React 接口。 */
export function useDesktopUpdate(): UpdateState {
  return React.useSyncExternalStore(subscribe, snapshot, snapshot);
}
