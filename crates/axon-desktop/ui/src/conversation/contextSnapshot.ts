/*
 * 会话上下文快照的视图模型。
 *
 * 头部占用、窗口分母与剩余量都读同一份快照：它是运行侧带时间戳的唯一来源。
 * 快照为 null 表示这条会话还没有任何可用读数，此时整段不占位，绝不拿上一次的旧值顶替；
 * 快照里缺的字段一律按未知处理，未知不是 0。
 *
 * 分母有一处受控回退：用量帧本身通常不带窗口字段，而窗口是**当前生效模型的配置事实**
 * （`viewConfig` 里由配置目录解析出的 `contextWindow`）。快照没带窗口时用这一份补，
 * 两者都没有才是未知。回退值必须是「这一轮真正在跑的模型」的窗口，不能拿配置默认模型
 * 的窗口冒充，否则会出现「模型是会话的、窗口却是默认模型的」读数。
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
  updatedAtMs: number | null;
  source: string | null;
}

/** 快照读取回执；`snapshot` 为空只表示还没有读数，与会话不存在不同。 */
export interface SessionContextSnapshot {
  sessionId: string;
  snapshot: ContextSnapshot | null;
}

/** 头部共用的读数。 */
export interface ContextSnapshotReading {
  /** 快照给出的当前占用；未知为空。 */
  usedTokens: number | null;
  /** 生效窗口：快照带的优先，缺失时回落到当前生效模型的配置事实。 */
  windowTokens: number | null;
  /** 窗口剩余；占用或窗口未知时为空。 */
  remainingTokens: number | null;
  /** 上下文构成估算；没有构成观测时为空。 */
  composition: ContextComposition | null;
  /** 「更新于 N 秒前」；`updatedAtMs` 缺失时是「未知更新时刻」。 */
  updatedText: string;
  /** 头部原生提示：口径与更新时刻。 */
  title: string;
}

/** 未知占位：分母未知时显示它，不显示 0。 */
export const UNKNOWN_TOKENS_LABEL = '未知';

/** 秒、分、时三档的毫秒基数。 */
const SECOND_MS = 1_000;
const MINUTE_MS = 60_000;
const HOUR_MS = 3_600_000;

/** 只有正数才算有效读数：0 与缺失同义，不是「观测到 0」。 */
function positiveTokens(value: number | null | undefined): number | null {
  return typeof value === 'number' && value > 0 ? value : null;
}

/**
 * 生效窗口：快照里的窗口优先，缺失时回落到当前生效模型配置里的窗口。
 *
 * 快照的窗口来自运行时实测（帧里带了就是权威）；模型配置里的窗口是这一轮请求真正按它
 * 算的预算，因此可以做分母。两者都没有时返回 null，界面显示未知。
 */
export function effectiveWindowTokens(
  snapshot: ContextSnapshot | null,
  configuredWindow: number | null | undefined,
): number | null {
  return positiveTokens(snapshot?.contextWindow) ?? positiveTokens(configuredWindow);
}

/** 已用读数文本：未知写「未知」。 */
export function usedTokensText(usedTokens: number | null): string {
  return usedTokens === null ? UNKNOWN_TOKENS_LABEL : formatDecimalCount(usedTokens);
}

/** 窗口读数文本：未知或非正数写「未知」。 */
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
 * 剩余量：模型窗口减去实测占用。
 *
 * 占用或窗口任一未知、或窗口非正数时返回 null，头部整段不显示；
 * 已用超过窗口时收敛到 0，不显示负数。
 */
export function remainingTokensOf(
  usedTokens: number | null,
  windowTokens: number | null,
): number | null {
  if (usedTokens === null || windowTokens === null || windowTokens <= 0) return null;
  return Math.max(0, windowTokens - usedTokens);
}

/**
 * 把快照读成界面读数。
 *
 * `configuredWindow` 是当前生效模型在配置目录里的窗口（见 `viewConfig` 的
 * `resolveEffectiveModel`）：快照没带窗口时用它做分母。占用只认快照，不拿窗口顶替。
 *
 * 返回 null 表示这份快照还承载不了「上下文占用」这一段（占用与窗口都未知）；
 * 此时头部整段不占位，而不是画一个 0。
 */
export function contextSnapshotReading(
  snapshot: ContextSnapshot | null,
  nowMs: number,
  configuredWindow: number | null = null,
): ContextSnapshotReading | null {
  if (snapshot === null) return null;
  const usedTokens = positiveTokens(snapshot.currentContextTokens);
  const windowTokens = effectiveWindowTokens(snapshot, configuredWindow);
  if (usedTokens === null && windowTokens === null) return null;
  const remainingTokens = remainingTokensOf(usedTokens, windowTokens);
  const updatedText = formatUpdatedAgo(snapshot.updatedAtMs, nowMs);
  const parts = [`上下文 ${contextPairText(usedTokens, windowTokens)}`];
  if (remainingTokens !== null) parts.push(`剩余 ${formatDecimalCount(remainingTokens)}`);
  parts.push(updatedText);
  return {
    usedTokens,
    windowTokens,
    remainingTokens,
    composition: snapshot.composition,
    updatedText,
    title: parts.join(' · '),
  };
}

/** 上下文占用比例（0~1）；窗口未知或无定义时为 null。 */
export function contextUsedRatio(reading: ContextSnapshotReading | null): number | null {
  if (reading === null) return null;
  if (reading.usedTokens === null || reading.usedTokens <= 0) return null;
  if (reading.windowTokens === null || reading.windowTokens <= 0) return null;
  return Math.min(1, reading.usedTokens / reading.windowTokens);
}
