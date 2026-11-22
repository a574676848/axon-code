/*
 * 运行态证据的结论口径。
 *
 * 「在跑」是网关给的事实，不是界面按投影推算出来的：进程消失、轮次租约过期、
 * provider 长时间不吐首字之后，投影里那一轮仍然停在流式中。界面能做的只有两件事：
 * 把证据的新鲜度说清楚，以及在证据不再成立时**停止宣称在跑**并给出可执行建议。
 *
 * 三种结论：
 * - `running`：证据成立，界面照常在跑；
 * - `expired`：界面认为在跑，但服务端的证据已经过期（租约到期、静默过久，或这一轮
 *   根本没有带租约的行），必须降级为「证据过期」；
 * - `unknown`：还没拿到任何证据（旧网关没有这个端点，或第一次读取尚未回来）。
 *   此时不改口，也不谎称过期。
 */

import { formatDurationMillisCompact } from './format';
import type { SessionLive } from './sessionEndpoints';

/** 静默超过这个时长，即使租约还挂着也不再宣称在跑。 */
export const LIVE_IDLE_THRESHOLD_MS = 120_000;

/** 连续读不到证据超过这个时长，就按「后台服务已停机」降级。 */
export const LIVE_UNREACHABLE_THRESHOLD_MS = 20_000;

/** 一次证据读数与它到手的时间。 */
export interface SessionLiveReading {
  /** 网关回报的证据。 */
  live: SessionLive;
  /** 读到这份证据的本地时刻（毫秒）。 */
  receivedAtMs: number;
  /** 连续读取失败从这一刻开始；一直读得到时为 null。 */
  unreachableSinceMs: number | null;
}

/** 证据结论。 */
export type LiveEvidenceState = 'running' | 'expired' | 'unknown';

/** 证据过期的卡片内容；未过期时各字段为空。 */
export interface LiveEvidence {
  /** 结论。 */
  state: LiveEvidenceState;
  /** 界面此刻还该不该显示「在跑」。 */
  showRunning: boolean;
  /** 卡片标题；仅过期时出现。 */
  title: string | null;
  /** 一句话结论；仅过期时出现。 */
  reason: string | null;
  /** 可执行建议；仅过期时出现。 */
  advice: string | null;
  /** 可执行建议对应的动作文案；仅过期时出现。 */
  action: string | null;
  /** 归因片段（租约余量、静默时长、尾序号）；仅过期时出现。 */
  meta: string[];
}

/** 证据过期卡片的固定标题与建议。 */
const EXPIRED_TITLE = '运行证据已过期';
const EXPIRED_ADVICE = '重启后台服务后再看这一轮；界面不再拿旧证据宣称在跑';
const RESTART_ACTION = '重启后台服务';

/** 空闲结论：不该显示任何卡片。 */
function idle(state: LiveEvidenceState, showRunning: boolean): LiveEvidence {
  return { state, showRunning, title: null, reason: null, advice: null, action: null, meta: [] };
}

/** 过期结论：标题、结论、建议与归因一起给出。 */
function expired(reason: string, meta: string[]): LiveEvidence {
  return {
    state: 'expired',
    showRunning: false,
    title: EXPIRED_TITLE,
    reason,
    advice: EXPIRED_ADVICE,
    action: RESTART_ACTION,
    meta,
  };
}

/**
 * 由「界面是否认为在跑」与「最近一份证据」得出结论。
 *
 * 界面不认为在跑时不读证据：没有在跑态就没有需要纠正的说法。
 * 界面认为在跑、却拿不到任何证据时给 `unknown`，保持原样而不是假装过期。
 */
export function liveEvidence(
  working: boolean,
  reading: SessionLiveReading | null,
  nowMs: number,
): LiveEvidence {
  if (!working) return idle('unknown', false);
  if (reading === null) return idle('unknown', true);
  const { live, receivedAtMs } = reading;
  const elapsedMs = Math.max(0, nowMs - receivedAtMs);
  const idleMs = live.idleForMs + elapsedMs;
  const sequenceMeta = `尾帧序号 ${live.lastFrameSequence}`;
  const idleMeta = `静默 ${formatDurationMillisCompact(live.idleForMs)}`;

  // 连续读不到证据：后台服务可能已经停机。这是最要紧的一条，先于租约判定。
  if (
    reading.unreachableSinceMs !== null &&
    nowMs - reading.unreachableSinceMs >= LIVE_UNREACHABLE_THRESHOLD_MS
  ) {
    return expired('后台服务已经不回应运行态查询，这一轮的证据无法再核对', [
      `已连续 ${formatDurationMillisCompact(nowMs - reading.unreachableSinceMs)} 读取失败`,
      sequenceMeta,
    ]);
  }

  // 服务端说这一轮没有带租约的执行者：这是最强的一条反证。
  if (!live.running) {
    return expired(
      '服务端没有带租约的在跑轮次，这一轮已经没有执行者',
      [idleMeta, sequenceMeta],
    );
  }
  const remainingMs = (live.leaseRemainingMs ?? 0) - elapsedMs;
  if (remainingMs <= 0) {
    return expired(
      '轮次租约已到期，这一轮没有执行者',
      [`租约余量 ${formatDurationMillisCompact(live.leaseRemainingMs ?? 0)} 已耗尽`, sequenceMeta],
    );
  }
  if (idleMs > LIVE_IDLE_THRESHOLD_MS) {
    return expired(
      `已经 ${formatDurationMillisCompact(idleMs)} 没有新事件，不再把它当作在跑`,
      [idleMeta, sequenceMeta],
    );
  }
  return idle('running', true);
}
