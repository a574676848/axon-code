/*
 * 一条 Interaction 的历史回放。
 *
 * 读的是持久日志的尾窗，写完就只订阅实时尾。三件事在这里定死：
 *
 * 1. 只读该交互自身的尾窗。首屏与「点开某一轮」同一口径：服务端按
 *    `(session_id, interaction_id)` 直接取最后 HISTORY_WINDOW_FRAMES 帧，
 *    不再先探整条会话的尾序号再算窗口起点。老交互的帧序号远小于整条会话的
 *    尾序号，旧算法会把整个老交互截空。
 * 2. 第一页落地就放行正文。过渡面等的是「有没有内容可看」，不是「整条读完了
 *    没有」；装载标志另走一条线，状态行照常写「正在加载会话」。
 * 3. 补窗不回订阅。往前补的那一段属于历史，接实时尾没有意义，也会把两路帧
 *    混进同一投影。
 *
 * 这一层不持有状态：状态更新一律经调用方给的回调，因此它能在没有 React 的
 * 地方被单独验证。
 */

import * as ipc from '../ipc';
import type { ConversationStreamEvent } from './wire';

/**
 * 一次读的窗口帧数。
 *
 * 桌面端单次历史帧页的上限是 1000（`MAX_HISTORY_LIMIT`），请求更大的值也会被
 * 截到同一条线上，所以这里直接按上限取：再多也只是把正文读进内存，接着被渲染
 * 窗口折掉。渲染这一侧同时最多挂 240 个单元（见 ConversationStream 的
 * RENDER_WINDOW），1000 帧按实测比例（476 帧约 60 格）约合 126 格，够填满一屏。
 *
 * 更早的帧不再翻页取：老交互的帧可能远在会话尾部之前，服务端会在尾窗扫空时
 * 退回全量扫描并保尾，客户端因此一次往返就能拿到可渲染的尾窗。
 *
 * 窗口起点之后可能没有用户那句话：回放前会按历史索引补一条种子气泡
 * （见 playNext 的 seed-history），所以「这一轮用户说了什么」始终读得到。
 */
export const HISTORY_WINDOW_FRAMES = 1_000;

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
 * 指时间线之上更早的轮次。窗口起点之前那些单元由渲染层折成一行提示，正文不再
 * 往回补：补进来的内容会立刻被同一个渲染预算折掉，等于白读一窗帧。
 */
export async function replayInteractionHistory(
  port: InteractionReplayPort,
  workspace: string,
  interactionId: string,
  generation: number,
): Promise<void> {
  const subscribeId = `interaction:${interactionId}`;
  const page = await ipc.conversationHistoryEvents(
    interactionId,
    0,
    HISTORY_WINDOW_FRAMES,
    true,
  );
  if (generation !== port.generation()) return;
  const settled = page.events.some((event) => isTerminal(event.eventType));
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
    port.painted();
  }
  port.drained();
  if (settled) {
    // 已经收口：不再订阅，直接把回放推进到下一条。
    port.advance(workspace, generation);
    return;
  }
  // 还在跑：从 durable 尾订阅，事件通道上只会出现真正的增量。
  port.subscribed(subscribeId);
  await port.watch(workspace, interactionId, page.tailCursor);
}
