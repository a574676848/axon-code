/*
 * 一条 Interaction 的历史帧读取。
 *
 * 正文的事实源只有一条：durable 帧。分页回放、展开与增量接管都从这里取帧，取回来一律
 * 交给与实时流同一个归约器（`applyFrames`），因此不存在第二套单元构造。
 *
 * 四件事在这里定死：
 *
 * 1. **两种读法，都按 durable 顺序交出帧**。首屏（打开会话、接管新轮）走「尾窗先行」：
 *    先用一次往返取该轮尾窗出屏，再在后台从头部按页补齐、整轮重投，超大轮次的首屏因此
 *    不再等串行往返；展开与折叠行仍走「从头按页推进」或只读一段，两处都要确定性而不是
 *    速度。无论哪条路，帧都按序号升序交出，缺段如实标记。
 * 2. **用量快照按流压缩**。一条 provider 流会连发几百上千条同形快照，而归约器只在流
 *    收口时读最后一条；逐帧投递等于把九成以上的解压与归约成本花在会被立刻覆盖的数据上。
 * 3. **页数有上限，且按轮次体量自适应**。补前段时先按「尾窗起点之前有多少帧」估出页数，
 *    再夹进硬上限；估算不足时如实标记中段被略过，绝不假装这一轮只有这些内容。
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
/**
 * 顺序补齐的页数上限。
 *
 * 一轮真的超过这个页数时，只交「已读的页 + 尾窗」并如实标记截断。旧实现按 40 页读满
 * 再丢掉中段，超长轮次的首屏因此被几十个串行往返拖住；这个上限把往返次数钉在
 * 上限 + 1 次（含尾窗），同时保住开头与结论。
 */
export const SEQUENTIAL_REPLAY_PAGES = 5;

/**
 * 补前段时允许的页数硬上限。
 *
 * 自适应的估算（见 [`replayPageBudget`]）在病态长轮次上会算出很大的页数；这个上限保证
 * 后台补读的往返次数有界，触顶时如实标记中段被略过。它只在后台发生，不挡首屏。
 */
export const REPLAY_MAX_PAGES = 12;

/**
 * 补前段要读多少页。
 *
 * 判据是「尾窗起点之前有多少帧」：事件序号是会话内单调的，尾窗首帧的序号就是它之前的
 * 帧数上界（这一轮的帧是其中的子集），除以页大小即所需页数。估算只是页数预算：
 * 多估不会多花往返（读到尾窗起点就停），少估才会撞上硬上限并如实标记截断。
 */
export function replayPageBudget(framesBeforeTail: number, pageSize: number): number {
  if (pageSize <= 0) return 1;
  const pages = Math.ceil(Math.max(0, framesBeforeTail) / pageSize);
  return Math.min(REPLAY_MAX_PAGES, Math.max(1, pages));
}

/**
 * 从 durable 游标里取事件序号；老记录的游标缺失或不是 JSON 时为空。
 *
 * 游标是 `{generation, sequence}` 的 JSON 文本：补前段要用序号判「读到哪一段为止」，
 * 缺它就只能退回「从头读到尾」，那正是尾窗先行要省掉的那部分代价。
 */
export function cursorSequence(cursor: string | null): number | null {
  if (cursor === null) return null;
  try {
    const parsed: unknown = JSON.parse(cursor);
    if (typeof parsed !== 'object' || parsed === null) return null;
    const sequence = (parsed as { sequence?: unknown }).sequence;
    return typeof sequence === 'number' && Number.isFinite(sequence) ? sequence : null;
  } catch {
    return null;
  }
}

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

// 单条 Interaction 的帧读取接缝在下面：尾窗先行（首屏）与从头推进（展开）共用一处取数口径。

/** 尾窗先行的选项。 */
export interface TailFirstReadOptions extends ReadFramesOptions {
  /**
   * 尾窗读回来时立刻回调：调用方在这里先出一屏。
   *
   * 回调里的帧只覆盖这一轮的最后一段；补齐之后同一个回调不会再被调用，整轮重投由
   * 返回值承担。
   */
  onTail: (read: InteractionFrames) => void;
}

/**
 * 首屏读法：先交尾窗，再在后台把前段补齐。
 *
 * 为什么这样做：超大轮次有上万帧，从头按页推进要让首屏等若干个串行往返（实测 1.5 至
 * 3.5 秒）；而读者打开会话时先要看的是**最近发生了什么**，更早的过程按需展开即可。
 * 尾窗一次往返就能把结论、终态与最近的工具行交出去，前段随后补齐并整轮重投，过程因此
 * 不再挡首屏。
 *
 * 尾窗不足一页说明这一轮的全部帧都在里面（网关按该轮自己的帧数保尾），此时不再补读，
 * 行为与一次整轮读完全一致。
 */
export async function readInteractionFramesTailFirst(
  interactionId: string,
  generation: number,
  options: TailFirstReadOptions,
): Promise<InteractionFrames | null> {
  const fetchPage: HistoryPageFetcher = options.fetchPage ?? ipc.conversationHistoryEvents;
  const stale = () => options.generation() !== generation;
  const limit = options.limit ?? HISTORY_PAGE_FRAMES;
  const subscribeId = `interaction:${interactionId}`;

  const tail = await fetchPage(interactionId, 0, limit, true);
  if (stale()) return null;
  const tailRead: InteractionFrames = {
    frames: compactUsageSnapshots(tail.events.map((frame) => streamFrame(subscribeId, frame))),
    cursor: tail.tailCursor,
    settled: tail.events.some((frame) => isTerminal(frame.eventType)),
    truncated: false,
  };
  options.onTail(tailRead);

  // 尾窗没被填满：这一轮的所有帧都已经在这里，不必再补前段。
  if (tail.events.length < limit) return tailRead;
  const firstSequence = cursorSequence(tail.events[0]?.cursor ?? null);
  if (firstSequence === null) return tailRead;

  const budget = replayPageBudget(firstSequence, limit);
  let head: ipc.HistoryFrame[] = [];
  let current = await fetchPage(interactionId, 0, limit, false);
  if (stale()) return null;
  let pages = 0;
  let complete = false;
  for (;;) {
    pages += 1;
    head = head.concat(current.events);
    // 已读到尾窗起点：前段与尾窗首尾相接，整轮齐了。
    if (current.nextAfter >= firstSequence) {
      complete = true;
      break;
    }
    if (!current.hasMore || pages >= budget) break;
    current = await fetchPage(interactionId, current.nextAfter, limit, false);
    if (stale()) return null;
  }

  const merged = mergeWithoutOverlap(head, tail.events);
  const streamed = compactUsageSnapshots(
    merged.map((frame) => streamFrame(subscribeId, frame)),
  );
  return {
    frames: streamed,
    cursor: tail.tailCursor,
    settled: streamed.some(isTerminalStreamEvent),
    truncated: !complete,
  };
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
  let frames = [...page.events];
  let cursor = page.tailCursor;
  let truncated = false;
  if (page.hasMore) {
    /*
     * 按页顺序补齐，到上限即停：到那时还没有读完，就再取一次尾窗，把「开头 + 已经读过
     * 的部分 + 结论」一起交出去，中段如实标记被略过。往返次数因此有上界，且不依赖
     * 会话级尾序号（那是整条会话的尾，不是这一轮自己的尾，拿来判断会把旧轮次误判成
     * 缺口过大而直接截断）。
     */
    let current = page;
    let pages = 1;
    while (current.hasMore && pages < SEQUENTIAL_REPLAY_PAGES) {
      current = await fetchPage(interactionId, current.nextAfter, limit, false);
      if (stale()) return null;
      frames = frames.concat(current.events);
      pages += 1;
    }
    cursor = current.tailCursor;
    if (current.hasMore) {
      const tail = await fetchPage(interactionId, 0, limit, true);
      if (stale()) return null;
      frames = mergeWithoutOverlap(frames, tail.events);
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
