/*
 * 会话上下文快照的视图模型。
 *
 * 头部占用、窗口分母与换窗读数都读同一份快照：它是运行侧带时间戳的唯一来源。
 * 快照为 null 表示这条会话还没有任何可用读数，此时整段不占位，绝不拿上一次的旧值
 * 或模型目录里的窗口去顶替；快照里缺的字段一律按未知处理，未知不是 0。
 */

import { formatDecimalCount } from './format';

/** 网关下发的上下文构成估算；缺项就是未知，不是 0。 */
export interface ContextComposition {
  systemPromptTokens?: number | null;
  toolDefinitionTokens?: number | null;
  conversationTokens?: number | null;
}

/** 网关下发的上下文快照；字段名与服务端契约逐字一致。 */
export interface ContextSnapshot {
  currentContextTokens: number | null;
  contextWindow: number | null;
  composition: ContextComposition | null;
  rolloverBeforeTokens: number | null;
  rolloverAfterTokens: number | null;
  updatedAtMs: number | null;
  source: string | null;
}

/** 快照读取回执；`snapshot` 为空只表示还没有读数，与会话不存在不同。 */
export interface SessionContextSnapshot {
  sessionId: string;
  snapshot: ContextSnapshot | null;
}

/** 头部与换窗行共用的读数。 */
export interface ContextSnapshotReading {
  /** 快照给出的当前占用；未知为空。 */
  usedTokens: number | null;
  /** 快照给出的窗口；现实中常为空。 */
  windowTokens: number | null;
  /** 上下文构成估算；没有构成观测时为空。 */
  composition: ContextComposition | null;
  /** 「更新于 N 秒前」；`updatedAtMs` 缺失时是「未知更新时刻」。 */
  updatedText: string;
  /** 换窗读数文本（`换窗前 825K · 换窗后 31.7K`）；没有换窗读数时为空。 */
  rolloverText: string | null;
  /** 头部原生提示：口径、更新时刻与同一份换窗数字。 */
  title: string;
}

/** 未知占位：分母未知时显示它，不显示 0。 */
export const UNKNOWN_TOKENS_LABEL = '未知';

/** 秒、分、时三档的毫秒基数。 */
const SECOND_MS = 1_000;
const MINUTE_MS = 60_000;
const HOUR_MS = 3_600_000;

/** 已用读数文本：未知写「未知」。 */
export function usedTokensText(usedTokens: number | null): string {
  return usedTokens === null ? UNKNOWN_TOKENS_LABEL : formatDecimalCount(usedTokens);
}

/**
 * 窗口读数文本：未知或非正数写「未知」。
 *
 * 用量帧通常不带窗口字段，此时分母就是未知。拿模型目录里的窗口值补上去会让
 * 「已用/窗口」看起来有分母，而那个分母并不是这一轮真的按它算的。
 */
export function windowTokensText(windowTokens: number | null): string {
  if (windowTokens === null || windowTokens <= 0) return UNKNOWN_TOKENS_LABEL;
  return formatDecimalCount(windowTokens);
}

/** 已用/窗口的展示文本。 */
export function contextPairText(usedTokens: number | null, windowTokens: number | null): string {
  return `${usedTokensText(usedTokens)}/${windowTokensText(windowTokens)}`;
}

/**
 * 更新时刻的相对说法。
 *
 * `updatedAtMs` 缺失时说「未知更新时刻」而不是「刚刚」：没有时间戳时我们并不知道
 * 这份读数有多旧，把它说成刚刚就是替运行侧撒谎。
 */
export function formatUpdatedAgo(updatedAtMs: number | null, nowMs: number): string {
  if (updatedAtMs === null) return '未知更新时刻';
  const ageMs = Math.max(0, nowMs - updatedAtMs);
  if (ageMs < SECOND_MS) return '更新于 1 秒内';
  if (ageMs < MINUTE_MS) return `更新于 ${Math.floor(ageMs / SECOND_MS)} 秒前`;
  if (ageMs < HOUR_MS) return `更新于 ${Math.floor(ageMs / MINUTE_MS)} 分钟前`;
  return `更新于 ${Math.floor(ageMs / HOUR_MS)} 小时前`;
}

/**
 * 换窗读数文本。
 *
 * 只读快照里的结构化读数：只有一侧可读时只写那一侧，两侧都未知时整段不出现。
 * 这是头部提示与换窗进度行共用的同一个函数，两处因此不会各读一套。
 */
export function rolloverTokensText(snapshot: ContextSnapshot | null): string | null {
  if (snapshot === null) return null;
  const before = snapshot.rolloverBeforeTokens;
  const after = snapshot.rolloverAfterTokens;
  const parts: string[] = [];
  if (before !== null && before > 0) parts.push(`换窗前 ${formatDecimalCount(before)}`);
  if (after !== null && after > 0) parts.push(`换窗后 ${formatDecimalCount(after)}`);
  return parts.length === 0 ? null : parts.join(' · ');
}

/**
 * 把快照读成界面读数。
 *
 * 返回 null 表示这份快照还承载不了「上下文占用」这一段（占用与窗口都未知，也没有
 * 换窗读数）；此时头部整段不占位，而不是画一个 0。
 */
export function contextSnapshotReading(
  snapshot: ContextSnapshot | null,
  nowMs: number,
): ContextSnapshotReading | null {
  if (snapshot === null) return null;
  const rolloverText = rolloverTokensText(snapshot);
  const usedTokens = snapshot.currentContextTokens;
  const windowTokens = snapshot.contextWindow;
  if (usedTokens === null && windowTokens === null && rolloverText === null) return null;
  const updatedText = formatUpdatedAgo(snapshot.updatedAtMs, nowMs);
  const pair = contextPairText(usedTokens, windowTokens);
  const title = [`上下文 ${pair}`, updatedText, rolloverText]
    .filter((part): part is string => part !== null)
    .join(' · ');
  return {
    usedTokens,
    windowTokens,
    composition: snapshot.composition,
    updatedText,
    rolloverText,
    title,
  };
}

/** 上下文占用比例（0~1）；窗口未知或无定义时为 null。 */
export function contextUsedRatio(reading: ContextSnapshotReading | null): number | null {
  if (reading === null) return null;
  if (reading.usedTokens === null || reading.usedTokens <= 0) return null;
  if (reading.windowTokens === null || reading.windowTokens <= 0) return null;
  return Math.min(1, reading.usedTokens / reading.windowTokens);
}

/**
 * 换窗进度行的读数文本。
 *
 * 有快照的换窗读数就用它：与头部提示里那两半数字是同一个函数产出的，两处不会各读一套。
 * 快照还没有换窗读数时保留运行侧自己的摘要（那句「当前上下文 N tokens」是它当时的记录），
 * 绝不为了对齐而把历史行的数字改写成最新一次的结果。
 */
export function rolloverLineSummary(runtimeSummary: string, rolloverText: string | null): string {
  return rolloverText ?? runtimeSummary;
}
