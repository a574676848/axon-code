/*
 * 一条 Interaction 的历史帧读取。
 *
 * 正文的事实源只有一条：durable 帧。分页回放、展开与增量接管都从这里取帧，取回来一律
 * 交给与实时流同一个归约器（`applyFrames`），因此不存在第二套单元构造。
 *
 * 四件事在这里定死：
 *
 * 1. **从头按页推进**，直到补到该 Interaction 的 durable 尾。只取尾窗会丢掉这一轮
 *    最前面的用户消息、提问与工具帧（推理总在最前面写），正文因此只剩最终一段。
 * 2. **用量快照按流压缩**。一条 provider 流会连发几百上千条同形快照，而归约器只在流
 *    收口时读最后一条；逐帧投递等于把九成以上的解压与归约成本花在会被立刻覆盖的数据上。
 * 3. **页数有上限**。逐页从头部读，超过上限就退化成「首页 + 尾窗」：首页保住这一轮的
 *    抬头（用户消息、提问、最早的动作），尾窗保住最终结论与终态帧，中间一段如实略过。
 * 4. **代次在每页之后复核**。目标已经切走时立刻放弃，半截正文不会落进新目标。
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
 * 单轮最多读多少页。
 *
 * 事件流里绝大多数帧是 `usageUpdate` 快照：实测一条 9.7 秒的轮次共 14543 帧，其中
 * 14006 帧（96.3%）是它，真正带正文的不足 4%。快照在本层按流压缩成最后一帧之后，
 * 页数才是仅剩的成本边界。四十页足够覆盖一条极长的轮次；超过就不再多读，改用
 * 「首页 + 尾窗」，绝不重演「只交尾窗、开头整段消失」。
 */
export const HISTORY_REPLAY_PAGES = 40;
/**
 * 允许顺序补齐的缺口上限（帧数）。
 *
 * 尾窗与首页之间的缺口不超过这个量级时，逐页读完更保真；超过就只交「首页 + 尾窗」并
 * 标记截断。四页是往返次数与保真度的折中：正常轮次根本走不到这里，超长轮次也不会
 * 被迫打几十个串行请求。
 */
export const SEQUENTIAL_REPLAY_FRAMES = HISTORY_PAGE_FRAMES * 4;

/** 用量快照帧在 wire 契约里的类型名。 */
const USAGE_WIRE_TYPE = 'usageUpdate';

/**
 * 用量快照帧的载荷标记。
 *
 * 分页一页就是上千帧，逐帧 `JSON.parse` 只为判断类型是净亏；wire 信封里类型只以
 * `"type":"usageUpdate"` 这一种写法出现，用标记直接判掉。判错的后果（漏压缩或误压缩）
 * 由 `interaction-replay` 的压缩等价用例兜住：压缩前后过同一个归约器必须深相等。
 */
const USAGE_PAYLOAD_MARKER = `"${USAGE_WIRE_TYPE}"`;

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

/** 是否为用量快照帧。 */
export function isUsageSnapshot(frame: ConversationStreamEvent): boolean {
  if (frame.kind !== 'frame') return false;
  return frame.eventType === 'wire_event' && frame.payloadJson.includes(USAGE_PAYLOAD_MARKER);
}

/** 是否为终态帧；订阅确认一类非帧事件一律不算。 */
function isTerminalStreamEvent(frame: ConversationStreamEvent): boolean {
  return frame.kind === 'frame' && isTerminal(frame.eventType);
}

/**
 * 把连续的用量快照压成最后一帧。
 *
 * 归约器按流记账：流内的快照是覆盖语义，只在流收口（`StreamingStarted` /
 * `StreamingCompleted`）时读最后一条。因此同一段连续快照里只留最后一条，喂进归约器的
 * 结果逐字段相同，而被丢掉的全是被后一条覆盖掉的中间态。
 */
export function compactUsageSnapshots(
  frames: readonly ConversationStreamEvent[],
): ConversationStreamEvent[] {
  const kept: ConversationStreamEvent[] = [];
  let pending: ConversationStreamEvent | null = null;
  for (const frame of frames) {
    if (isUsageSnapshot(frame)) {
      pending = frame;
      continue;
    }
    if (pending !== null) {
      kept.push(pending);
      pending = null;
    }
    kept.push(frame);
  }
  if (pending !== null) kept.push(pending);
  return kept;
}

/**
 * 首页与尾窗按游标合并。
 *
 * 两段正常情况下互不相交；窄轮次里尾窗会与首页重叠，因此按游标去重，绝不把同一帧
 * 投两遍（推理帧是追加语义，叠一遍就是重复正文）。
 */
function mergeWithoutOverlap(
  head: readonly ipc.HistoryFrame[],
  tail: readonly ipc.HistoryFrame[],
): ipc.HistoryFrame[] {
  const merged = [...head];
  const seen = new Set<string>();
  for (const frame of head) {
    if (frame.cursor !== null) seen.add(frame.cursor);
  }
  for (const frame of tail) {
    if (frame.cursor !== null) {
      if (seen.has(frame.cursor)) continue;
      seen.add(frame.cursor);
    }
    merged.push(frame);
  }
  return merged;
}

/** 一轮帧的读取结论。 */
export interface InteractionFrames {
  /** 可直接喂给 `applyFrames` 的帧，按 durable 顺序。 */
  frames: ConversationStreamEvent[];
  /** 订阅实时尾用的游标；没有可订阅的尾时为空。 */
  cursor: string | null;
  /** 这一轮是否已经收口。 */
  settled: boolean;
  /** 中段是否被页数上限略过：界面据此如实说明，而不是假装这一轮只有这些内容。 */
  truncated: boolean;
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
    const frames = compactUsageSnapshots(
      tail.events.map((frame) => streamFrame(subscribeId, frame)),
    );
    return {
      frames,
      cursor: tail.tailCursor,
      settled: frames.some(isTerminalStreamEvent),
      truncated: tail.hasMore,
    };
  }

  if (options.headOnly === true) {
    const head = await fetchPage(interactionId, 0, limit, false);
    if (stale()) return null;
    const frames = compactUsageSnapshots(
      head.events.map((frame) => streamFrame(subscribeId, frame)),
    );
    return {
      frames,
      cursor: head.tailCursor,
      settled: frames.some(isTerminalStreamEvent),
      truncated: head.hasMore,
    };
  }

  const page = await fetchPage(interactionId, 0, limit, false);
  if (stale()) return null;
  const first = page;
  let frames = [...page.events];
  let cursor = page.tailCursor;
  let truncated = false;
  if (page.hasMore) {
    /*
     * 先用首页带回的 durable 尾定界，再决定要不要逐页读完整轮。
     *
     * 缺口只有几页时顺序读完更保真；缺口很大时只交「首页 + 尾窗」两次往返，中间一段
     * 如实标记为略过。旧实现无论缺口多大都先逐页读到上限（40 页）再丢掉中间、回头取
     * 尾窗，等于先白读几十页：超长轮次的首屏因此被几十个串行往返拖住。
     */
    const gapFrames = Math.max(0, page.tailSequence - page.nextAfter);
    if (gapFrames <= SEQUENTIAL_REPLAY_FRAMES) {
      let current = page;
      let pages = 1;
      while (current.hasMore && pages < HISTORY_REPLAY_PAGES) {
        current = await fetchPage(interactionId, current.nextAfter, limit, false);
        if (stale()) return null;
        frames = frames.concat(current.events);
        pages += 1;
      }
      cursor = current.tailCursor;
      if (current.hasMore) {
        const tail = await fetchPage(interactionId, 0, limit, true);
        if (stale()) return null;
        frames = mergeWithoutOverlap(first.events, tail.events);
        cursor = tail.tailCursor;
        truncated = true;
      }
    } else {
      const tail = await fetchPage(interactionId, 0, limit, true);
      if (stale()) return null;
      frames = mergeWithoutOverlap(first.events, tail.events);
      cursor = tail.tailCursor;
      truncated = true;
    }
  }
  const streamed = compactUsageSnapshots(
    frames.map((frame) => streamFrame(subscribeId, frame)),
  );
  return {
    frames: streamed,
    cursor,
    settled: streamed.some(isTerminalStreamEvent),
    truncated,
  };
}
