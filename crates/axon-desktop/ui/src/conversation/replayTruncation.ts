/*
 * 截断可见与按序号续读。
 *
 * 网关在订阅确认帧里给出两件事：这次 durable 回放**没有追到尾巴**，以及真实的续读游标。
 * 客户端必须两件都消费：把「历史是残缺的」说出来，并从游标继续补读；把残缺回放当完整
 * 历史渲染，或从 0 重放一遍，都是在制造一份看不出问题的假历史。
 *
 * 这里只有判定与文案，不做 IO：续订由会话编排按 `replayResume` 的结论发出。
 */

import { cursorSequence } from './durableCursor';

/** 订阅确认帧里的截断事实。 */
export interface ReplayReceipt {
  /** 这次回放是否没有追到 durable 尾。 */
  replayTruncated: boolean;
  /** 真实续读位置（网关的 durable 游标 JSON 文本）；未截断时为空。 */
  resumeCursor: string | null;
}

/** 续读决策。 */
export interface ReplayResume {
  /** 用来续订的游标：服务端给的真实位置，不是 0。 */
  resumeCursor: string;
  /** 游标序号；解析不出来时为 null（仍按原游标续订，只是说不出「第 N 帧」）。 */
  sequence: number | null;
}

/** 游标序号的解析与历史回放共用同一份实现，避免两处各写一套。 */
export { cursorSequence } from './interactionReplay';

/**
 * 从确认帧得出续读决策。
 *
 * 未截断、或截断却没有游标时返回 null：前者已经读到尾，后者没有可续的位置，
 * 此时界面只能如实说残缺，不能自己编一个起点。
 */
export function replayResume(receipt: ReplayReceipt): ReplayResume | null {
  if (!receipt.replayTruncated) return null;
  const cursor = receipt.resumeCursor;
  if (cursor === null || cursor.length === 0) return null;
  return { resumeCursor: cursor, sequence: cursorSequence(cursor) };
}

/**
 * 残缺历史的提示文案。
 *
 * 序号取自续读游标，因此「显示到第几帧」就是服务端已经确认读到的那一帧；
 * 说不出序号时只说明残缺，不编帧号。
 */
export function replayTruncationNotice(receipt: ReplayReceipt): string | null {
  if (!receipt.replayTruncated) return null;
  const sequence = cursorSequence(receipt.resumeCursor);
  if (sequence === null) return '历史回放未追到最新，界面显示的不是完整历史';
  return `历史仅显示到第 ${sequence} 帧`;
}

/**
 * 一帧是否还该落进投影。
 *
 * 续订会从同一个订阅的游标之后重新回放，网关也会把实时帧与回放帧叠在一起；按订阅
 * 内的序号去重，重叠帧因此不会长出重复气泡。没有游标的帧（瞬态帧）一律放行。
 */
export function acceptsFrameSequence(previous: number | null, sequence: number | null): boolean {
  if (sequence === null) return true;
  return previous === null || sequence > previous;
}

/** 订阅确认帧上的截断事实；旧网关缺字段时按「读到尾」处理。 */
export function replayReceiptOf(subscribed: {
  replayTruncated?: boolean;
  resumeCursor?: string | null;
}): ReplayReceipt {
  return {
    replayTruncated: subscribed.replayTruncated === true,
    resumeCursor: subscribed.resumeCursor ?? null,
  };
}
