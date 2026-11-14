/*
 * 一条 Interaction 的历史回放。
 *
 * 读的是持久日志的帧页，写完就只订阅实时尾。三件事在这里定死：
 *
 * 1. 一律读尾部一窗。首屏与「点开某一轮」同一口径：一条几十万帧的长轮次全量回放
 *    要几百次串行往返，而读者要看的是尾部。更早的部分按窗往前补（见 earlierWindowStart）。
 * 2. 第一页落地就放行正文。过渡面等的是「有没有内容可看」，不是「整条读完了没有」；
 *    装载标志另走一条线，状态行照常写「正在加载会话」。
 * 3. 补窗不回订阅。往前补的那一段属于历史，接实时尾没有意义，也会把两路帧混进同一投影。
 *
 * 这一层不持有状态：状态更新一律经调用方给的回调，因此它能在没有 React 的地方被
 * 单独验证。
 */

import * as ipc from '../ipc';
import type { ConversationStreamEvent } from './wire';

/**
 * 一次读的窗口帧数。
 *
 * 渲染这一侧同时最多挂 240 个单元（见 ConversationStream 的 RENDER_WINDOW），
 * 两千帧按实测比例（476 帧约 60 格）刚好填满还有富余；再大的窗口只是把正文
 * 读进内存，接着被渲染窗口折掉，白白多走几次帧页。
 *
 * 窗口起点之后可能没有用户那句话：回放前会按历史索引补一条种子气泡
 * （见 playNext 的 seed-history），所以「这一轮用户说了什么」始终读得到。
 */
export const HISTORY_WINDOW_FRAMES = 2_000;

/** 单次历史帧页的帧数：网关允许的上限，往返次数因此最少。 */
export const HISTORY_PAGE_FRAMES = 1_000;

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
  /** 补到 durable 尾：此后是实时尾，正文不再是「正在加载」。 */
  drained: () => void;
  /** 登记订阅标识。 */
  subscribed: (subscribeId: string) => void;
  /** 从尾游标订阅增量。 */
  watch: (workspace: string, interactionId: string, cursor: string | null) => Promise<void>;
  /** 已经收口：推进到下一个目标。 */
  advance: (workspace: string, generation: number) => void;
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
 * 回放一条 Interaction 的尾部一窗历史帧，并从尾游标接上实时增量。
 *
 * 首屏与展开同一口径：只读尾部一窗，更早的部分不提供加载入口：「加载更早」始终
 * 指时间线之上更早的轮次。窗口起点之前那些单元由渲染层折成一行提示，正文不再往回补：
 * 补进来的内容会立刻被同一个渲染预算折掉，等于白读一窗帧。
 */
export async function replayInteractionHistory(
  port: InteractionReplayPort,
  workspace: string,
  interactionId: string,
  generation: number,
): Promise<void> {
  const subscribeId = `interaction:${interactionId}`;
  const limit = (
    await ipc.conversationHistoryEvents(interactionId, Number.MAX_SAFE_INTEGER, 1)
  ).tailSequence;
  if (generation !== port.generation()) return;
  const windowStart = earlierWindowStart(limit);
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
    // 窗口边界到了就停：补窗只读这一段，尾部窗口读到 durable 尾为止。
    if (after >= limit) break;
    if (!page.hasMore) break;
  }
  if (generation !== port.generation()) return;
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

/** 往前补一窗的起点；返回 0 表示已经到了这条 Interaction 的开头。 */
export function earlierWindowStart(beforeSequence: number): number {
  return Math.max(0, beforeSequence - HISTORY_WINDOW_FRAMES);
}
