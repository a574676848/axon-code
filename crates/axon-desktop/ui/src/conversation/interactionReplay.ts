/*
 * 一条 Interaction 的历史回放。
 *
 * 读的是持久日志的帧页，写完就只订阅实时尾。两件事在这里定死：
 *
 * 1. 首屏只读尾部窗口。一条几十万帧的长轮次全量回放要几百次串行往返，而首屏真正
 *    要看的是尾部；更早的过程留给「加载更早」显式补。
 * 2. 第一页落地就放行正文。过渡面等的是「有没有内容可看」，不是「整条读完了没有」；
 *    装载标志另走一条线，状态行照常写「正在加载会话」。
 *
 * 这一层不持有状态：状态更新一律经调用方给的回调，因此它能在没有 React 的地方被
 * 单独验证。
 */

import * as ipc from '../ipc';
import type { ConversationStreamEvent } from './wire';

/** 首屏只读尾部这么多帧（约一个长轮次的量级）。 */
export const HISTORY_WINDOW_FRAMES = 12_000;

/** 单次历史帧页的帧数：网关允许的上限，往返次数因此最少。 */
export const HISTORY_PAGE_FRAMES = 1_000;

/** 首屏窗口的截断记录：哪一条、从哪个序号读起。 */
export interface TruncatedWindow {
  interactionId: string;
  start: number;
}

/** 回放用的接缝。 */
export interface InteractionReplayPort {
  /** 一页帧的落点：主投影或分页临时投影。 */
  route: (frames: ConversationStreamEvent[], nowMs: number) => void;
  /** 帧派发时用的时间戳。 */
  now: () => number;
  /** 当前代次；与调用时的代次不一致即整次回放作废。 */
  generation: () => number;
  /** 第一页落地：正文可以交出去了。 */
  painted: () => void;
  /** 记下这一次从哪个起点读起；起点为 0 表示整条读全。 */
  truncated: (interactionId: string, start: number, live: boolean) => void;
  /** 补到 durable 尾：此后是实时尾，正文不再是「正在加载」。 */
  drained: () => void;
  /** 登记订阅标识。 */
  subscribed: (subscribeId: string) => void;
  /** 从尾游标订阅增量。 */
  watch: (workspace: string, interactionId: string, cursor: string | null) => Promise<void>;
  /** 已经收口：推进到下一条。 */
  advance: (workspace: string, generation: number) => void;
}

/**
 * 首屏窗口起点：先探一次 durable 尾，只保留最后一屏帧。
 *
 * 探测页只用来拿尾序号，它的事件一律丢弃：补到尾的那一页会把终态帧一起带回来，
 * 把它当正文派发会把还没收口的轮次提前判成结束。
 */
export async function firstWindowStart(interactionId: string): Promise<number> {
  const probe = await ipc.conversationHistoryEvents(interactionId, Number.MAX_SAFE_INTEGER, 1);
  return Math.max(0, probe.tailSequence - HISTORY_WINDOW_FRAMES);
}

/** 终态帧：只会在补到 durable 尾的那一页出现。 */
function isTerminal(eventType: string): boolean {
  return (
    eventType === 'turn_completed' ||
    eventType === 'turn_failed' ||
    eventType === 'interaction.status'
  );
}

/**
 * 回放一条 Interaction 的历史帧，并从尾游标接上实时增量。
 *
 * `full` 为真时从 0 读起（展开折叠轮次、补回被窗口截掉的那一条），否则只读尾部窗口。
 */
export async function replayInteractionHistory(
  port: InteractionReplayPort,
  workspace: string,
  interactionId: string,
  generation: number,
  full: boolean,
): Promise<void> {
  const subscribeId = `interaction:${interactionId}`;
  const windowStart = full ? 0 : await firstWindowStart(interactionId);
  if (generation !== port.generation()) return;
  let after = windowStart;
  let tailCursor: string | null = null;
  let settled = false;
  let painted = false;
  for (;;) {
    const page = await ipc.conversationHistoryEvents(interactionId, after, HISTORY_PAGE_FRAMES);
    if (generation !== port.generation()) return;
    if (page.events.length > 0) {
      port.route(
        page.events.map((event) => ({
          kind: 'frame' as const,
          subscribeId,
          eventType: event.eventType,
          payloadJson: event.payloadJson,
          cursor: event.cursor,
        })),
        port.now(),
      );
      if (!painted) {
        painted = true;
        port.painted();
      }
      settled = page.events.some((event) => isTerminal(event.eventType));
    }
    tailCursor = page.tailCursor;
    after = page.nextAfter;
    if (!page.hasMore) break;
  }
  if (generation !== port.generation()) return;
  port.truncated(interactionId, windowStart, !settled);
  port.drained();
  if (settled) {
    // 已经收口：不再订阅，直接把回放推进到下一条。
    port.advance(workspace, generation);
    return;
  }
  // 还在跑：从 durable 尾订阅，事件通道上只会出现真正的增量。
  port.subscribed(subscribeId);
  await port.watch(workspace, interactionId, tailCursor);
}
