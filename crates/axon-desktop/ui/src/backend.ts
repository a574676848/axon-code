import { backendError, isBackendUnavailable } from './errors';
import type { GatewayHealth } from './types';

/*
 * 后端调用的传输层。
 *
 * 桌面窗口优先走 Tauri IPC；浏览器调试走本地调试桥。两条通道调用的是同一批后端
 * 函数，读写对象都是真实的 ~/.axon 数据，因此这里不存在任何伪造数据或离线回退。
 *
 * 与具体命令的封装分开：命令按领域写在 `ipc.ts`（心跳、会话、工作区……），
 * 这一层只负责「把一次调用送出去、把失败分级」。全部后端调用都经过 `call`，
 * 因此「后端服务未启用」只需在这一处判定：命中即统一通知一次，由外壳重新探测
 * 并弹出既有的统一弹窗，各视图不必各自解释同一个断连原因，也不会出现
 * 「有的地方报错、有的地方静默」。
 */

/** 本地调试桥默认端口，与 `--serve` 的默认值保持一致。 */
export const BRIDGE_PORT = 14222;

/** 当前是否运行在桌面宿主内。 */
export const isDesktopHost = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/*
 * 判据分两层，先快后准：
 * 1. 传输层文案（「网关不可达」等）是断连的直接证据，命中即通知，不再多探一次；
 * 2. 其余失败做一次**事实核验**：让健康探测自己回答服务是否可用。业务拒绝
 *    （凭据、状态码、归属不符）会被探测回答「已连接」，因此不会误弹统一弹窗，
 *    判断也不再依赖错误措辞。
 */
const backendUnavailableListeners = new Set<(detail: string) => void>();

/**
 * 订阅「后端服务未启用」；回调带上触发这次判定的失败原因，供统一弹窗直接展示。
 *
 * 返回取消订阅函数。
 */
export function onBackendUnavailable(listener: (detail: string) => void): () => void {
  backendUnavailableListeners.add(listener);
  return () => {
    backendUnavailableListeners.delete(listener);
  };
}

/**
 * 通知节流窗口。
 *
 * 探测本身在服务没起来时也会失败，没有这个窗口就会形成
 * 「请求失败 → 通知 → 重新探测 → 再失败」的自激循环。
 */
const UNAVAILABLE_NOTIFY_INTERVAL_MS = 1000;
let lastUnavailableNotifyAtMs = 0;

function notifyBackendUnavailable(detail: string): void {
  const now = Date.now();
  if (now - lastUnavailableNotifyAtMs < UNAVAILABLE_NOTIFY_INTERVAL_MS) return;
  lastUnavailableNotifyAtMs = now;
  for (const listener of backendUnavailableListeners) listener(detail);
}

/**
 * 事实核验的节流窗口，比通知窗口宽。
 *
 * 一次业务拒绝就探一次是浪费；窗口内已经探明「服务可用」的结论同样适用于紧随
 * 其后的失败，因此窗口内直接复用结论，不再重复探测。
 */
const UNAVAILABLE_PROBE_INTERVAL_MS = 1200;
let lastProbeAtMs = 0;

/** 复探一次运行态服务；不可达或探测本身失败都算「未启用」。 */
async function probeBackendUnavailable(): Promise<boolean> {
  const now = Date.now();
  if (now - lastProbeAtMs < UNAVAILABLE_PROBE_INTERVAL_MS) return false;
  lastProbeAtMs = now;
  try {
    const health = await callRaw<GatewayHealth>('gateway_health');
    return !health.connected;
  } catch {
    // 探测都发不出去（桌面宿主缺失、调试桥没起）同样是「未启用」。
    return true;
  }
}

/**
 * 拦截一次调用失败。
 *
 * 不阻塞调用方：事实核验在后台完成，失败原因照常、立刻回给调用方，
 * 因此错误提示不会因为多了一次探测而变慢。
 */
function interceptBackendUnavailable(failure: unknown): void {
  const detail = backendError(failure).message;
  if (isBackendUnavailable(failure)) {
    notifyBackendUnavailable(detail);
    return;
  }
  void probeBackendUnavailable().then((unavailable) => {
    if (unavailable) notifyBackendUnavailable(detail);
  });
}

/**
 * 统一的原始调用入口；只负责传输，不做任何拦截。
 *
 * 拦截器自己要用它做健康复探：复探若再走 `call`，探测失败会再触发探测。
 */
export async function callRaw<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  if (isDesktopHost()) {
    const internals = (
      window as unknown as {
        __TAURI_INTERNALS__: {
          invoke: (cmd: string, args?: Record<string, unknown>) => Promise<T>;
        };
      }
    ).__TAURI_INTERNALS__;
    return internals.invoke(command, args);
  }

  let response: Response;
  try {
    response = await fetch(`http://127.0.0.1:${BRIDGE_PORT}/rpc`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ cmd: command, args }),
    });
  } catch {
    throw new Error(
      `未连接到桌面宿主，也没有检测到本地调试桥（127.0.0.1:${BRIDGE_PORT}）。请启动桌面应用，或以 --serve 参数启动调试桥。`,
    );
  }
  if (!response.ok) {
    throw new Error(`本地调试桥响应异常：HTTP ${response.status}`);
  }
  const payload = (await response.json()) as { ok: boolean; data?: T; error?: string };
  if (!payload.ok) {
    throw new Error(payload.error ?? '本地调试桥返回失败');
  }
  return payload.data as T;
}

/** 统一的后端调用入口：所有失败都过一遍拦截器，错误原样抛出。 */
export async function call<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  try {
    return await callRaw<T>(command, args);
  } catch (failure) {
    const error = backendError(failure);
    interceptBackendUnavailable(error);
    throw error;
  }
}
