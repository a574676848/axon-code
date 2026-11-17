/*
 * 一条 Interaction 的历史帧读取。
 *
 * 正文的事实源只有一条：durable 帧。分页回放、展开与增量接管都从这里取帧，取回来一律
 * 交给与实时流同一个归约器（`applyFrames`），因此不存在第二套单元构造。
 *
 * 三件事在这里定死：
 *
 * 1. **从头按页推进**，直到补到该 Interaction 的 durable 尾。只取尾窗会丢掉这一轮
 *    最前面的思考与工具帧（推理总在最前面写），正文因此只剩最终一段。
 * 2. **单轮预算**。逐页从头部读，超过预算就退化成一次尾窗读取：宁可少一段过程，
 *    也不能把打开会话拖成几十次串行往返。退化时只投递尾窗，绝不把已经读过的头部
 *    与尾窗叠着投两遍（推理帧是追加语义，叠一遍就是重复正文）。
 * 3. **代次在每页之后复核**。目标已经切走时立刻放弃，半截正文不会落进新目标。
 *
 * 这一层不持有状态、不碰 React，分页口径由 `tests/interaction-replay.test.mjs` 钉住。
 */

import * as ipc from '../ipc';
import type { ConversationStreamEvent } from './wire';

/**
 * 一次读的帧数。
 *
 * 桌面端单次历史帧页的上限是 1000（`MAX_EVENT_PAGE`），请求更大的值也会被截到同一条
 * 线上，所以这里直接按上限取。
 */
export const HISTORY_PAGE_FRAMES = 1_000;

/**
 * 单轮的帧预算。
 *
 * 实测一条 40 轮的车道共 12.4 万帧，摊到单轮约 3 千帧，四千帧足够覆盖绝大多数轮次；
 * 超过预算的轮次退化成尾窗读取，打开会话的网络与内存开销因此有确定的上界。
 */
export const HISTORY_REPLAY_BUDGET = 4_000;

/** 历史帧页的来源；缺省走 IPC，测试注入替身以钉住分页口径。 */
export type HistoryPageFetcher = (
  interactionId: string,
  after: number,
  limit: number,
  tail: boolean,
) => Promise<ipc.HistoryPage>;

/** 终态帧：只会在这一轮真正收口之后出现。 */
function isTerminal(eventType: string): boolean {
  return (
    eventType === 'turn_completed' ||
    eventType === 'turn_failed' ||
    eventType === 'interaction.status'
  );
}

/** 一帧历史帧换成事件通道上的同形帧：字段逐项对齐，前端只认一套解析。 */
function streamFrame(
  subscribeId: string,
  frame: ipc.HistoryFrame,
): ConversationStreamEvent {
  return {
    kind: 'frame',
    subscribeId,
    eventType: frame.eventType,
    payloadJson: frame.payloadJson,
    cursor: frame.cursor,
  };
}

/** 一轮帧的读取结论。 */
export interface InteractionFrames {
  /** 可直接喂给 `applyFrames` 的帧，按 durable 顺序。 */
  frames: ConversationStreamEvent[];
  /** 订阅实时尾用的游标；没有可订阅的尾时为空。 */
  cursor: string | null;
  /** 这一轮是否已经收口。 */
  settled: boolean;
}

/** 读帧的接缝。 */
export interface ReadFramesOptions {
  /** 当前代次；与调用时的代次不一致即整次读取作废。 */
  generation: () => number;
  /** 历史帧页来源；缺省走 IPC。 */
  fetchPage?: HistoryPageFetcher;
  /**
   * 只读尾窗。
   *
   * 折叠行只要「这一轮最近发生过什么」：工具类别摘要与收口结论都在尾部，而整轮读会
   * 为一条折叠行付出几十次往返。展开才读整轮（`tailOnly` 为假）。
   */
  tailOnly?: boolean;
  /** 只读第一页（含过程的头部）：折叠行只要工具类别，不把整轮拖回来。 */
  headOnly?: boolean;
  /** 单页帧数上限；缺省 `HISTORY_PAGE_FRAMES`。 */
  limit?: number;
}

/**
 * 读一条 Interaction 的帧。
 *
 * 两种读法共用一处取数口径：整轮（首屏最新一轮与展开）按页推进到它自己的 durable 尾，
 * 尾窗（折叠行）只取最后一小段。帧一律换成事件通道同形帧，调用方直接喂 `applyFrames`。
 */
export async function readInteractionFrames(
  interactionId: string,
  generation: number,
  options: ReadFramesOptions,
): Promise<InteractionFrames | null> {
  const fetchPage: HistoryPageFetcher = options.fetchPage ?? ipc.conversationHistoryEvents;
  const stale = () => options.generation() !== generation;
  const limit = options.limit ?? HISTORY_PAGE_FRAMES;
  const subscribeId = `interaction:${interactionId}`;

  if (options.tailOnly === true) {
    const tail = await fetchPage(interactionId, 0, limit, true);
    if (stale()) return null;
    return {
      frames: tail.events.map((frame) => streamFrame(subscribeId, frame)),
      cursor: tail.tailCursor,
      settled: tail.events.some((frame) => isTerminal(frame.eventType)),
    };
  }

  if (options.headOnly === true) {
    const head = await fetchPage(interactionId, 0, limit, false);
    if (stale()) return null;
    return {
      frames: head.events.map((frame) => streamFrame(subscribeId, frame)),
      cursor: head.tailCursor,
      settled: head.events.some((frame) => isTerminal(frame.eventType)),
    };
  }

  let page = await fetchPage(interactionId, 0, limit, false);
  if (stale()) return null;
  let frames = [...page.events];
  let read = frames.length;
  while (page.hasMore && read < HISTORY_REPLAY_BUDGET) {
    page = await fetchPage(interactionId, page.nextAfter, limit, false);
    if (stale()) return null;
    frames = frames.concat(page.events);
    read += page.events.length;
  }
  let cursor = page.tailCursor;
  if (page.hasMore) {
    // 单轮帧数超过预算：只保留尾窗（其中一定有终态与最终正文），并改用它的订阅游标。
    const tail = await fetchPage(interactionId, 0, limit, true);
    if (stale()) return null;
    frames = [...tail.events];
    cursor = tail.tailCursor;
  }
  return {
    frames: frames.map((frame) => streamFrame(subscribeId, frame)),
    cursor,
    settled: frames.some((frame) => isTerminal(frame.eventType)),
  };
}
