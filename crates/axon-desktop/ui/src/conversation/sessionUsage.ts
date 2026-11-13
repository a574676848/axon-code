/*
 * 会话级预算读数：上下文与窗口占用取「最近一次请求」，会话累计取整条会话。
 *
 * 新一轮刚提交、Provider 还没回报用量时，这一轮的用量是空的；直接采用它，指标条就会
 * 在换轮那一刻掉回 0/窗口，读者只会读成「数据被清零」。上下文读数是会话事实而不是
 * 单轮事实，因此从后往前取第一条有观测的用量；整条会话一次都没观测过时才返回 null，
 * 此时指标条按零读数据渲染（新建会话的 `0/窗口`）。
 *
 * 会话累计另有两条来源：历史会话正文是逐条回放的，界面并不知道更早的轮次花了多少，
 * 因此基线由网关按车道聚合后随历史页一起下发，本地只叠加基线取回之后发生的轮次。
 */

import { currentContextTokens, totalTokens, type TokenUsage } from './transcript';
import type { TurnState } from './transcript/state';

/** 一条会话的累计用量分桶，与网关下发的形状一致。 */
export interface SessionUsageTotals {
  /** 总量。 */
  totalTokens: number;
  /** 未缓存输入。 */
  inputTokens: number;
  /** 缓存读取。 */
  cacheReadTokens: number;
  /** 缓存写入。 */
  cacheWriteTokens: number;
  /** 输出。 */
  outputTokens: number;
  /** 推理输出。 */
  reasoningTokens: number;
}

/** 空用量：新建会话、基线全零时用它。 */
export const EMPTY_SESSION_USAGE: SessionUsageTotals = {
  totalTokens: 0,
  inputTokens: 0,
  cacheReadTokens: 0,
  cacheWriteTokens: 0,
  outputTokens: 0,
  reasoningTokens: 0,
};

/** 这一轮的用量是否已经有观测：任一桶非零即视为 Provider 报过用量。 */
export function hasUsageObservation(usage: TokenUsage): boolean {
  return totalTokens(usage) > 0 || currentContextTokens(usage) > 0;
}

/** 最近一次有观测的轮次用量；整条会话都没有观测时为 null。 */
export function lastObservedUsage(turns: readonly { usage: TokenUsage }[]): TokenUsage | null {
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const usage = turns[index]?.usage;
    if (usage !== undefined && hasUsageObservation(usage)) return usage;
  }
  return null;
}

/**
 * 会话累计读数的基线。
 *
 * `usage` 与 `elapsedMs` 是网关按整条车道聚合出的已收口轮次读数；`turnIds` 是它覆盖到的
 * 那些轮次，判定用轮次标识（回放出来的轮次以 Interaction 标识为轮次标识），本地据此避免
 * 把同一轮既算进基线又算进增量。
 */
export interface SessionTokenBaseline {
  /** 基线覆盖的累计用量。 */
  usage: SessionUsageTotals;
  /** 基线覆盖的墙钟耗时（毫秒）。 */
  elapsedMs: number;
  /** 基线覆盖到的轮次标识。 */
  turnIds: ReadonlySet<string>;
}

/** 会话累计读数组。 */
export interface SessionCumulative extends SessionUsageTotals {
  /** 整条会话累计的墙钟耗时（毫秒）。 */
  elapsedMs: number;
}

/** 空集合：调用方不区分「还没有迟到轮次」与「没有迟到轮次」时用它。 */
const NO_TURNS: ReadonlySet<string> = new Set<string>();

/** 把一轮的用量加进累计。 */
function addUsage(total: SessionUsageTotals, usage: TokenUsage): SessionUsageTotals {
  return {
    totalTokens: total.totalTokens + totalTokens(usage),
    inputTokens: total.inputTokens + usage.inputTokens,
    cacheReadTokens: total.cacheReadTokens + usage.cacheReadInputTokens,
    cacheWriteTokens: total.cacheWriteTokens + usage.cacheWriteInputTokens,
    outputTokens: total.outputTokens + usage.outputTokens,
    reasoningTokens: total.reasoningTokens + usage.reasoningOutputTokens,
  };
}

/**
 * 整条会话截至当前的累计读数（用量与耗时）。
 *
 * 两段合成：基线负责打开会话之前就已收口的轮次，本地增量负责基线取回之后才发生的轮次
 * （含正在生成的那一轮，因此读数边跑边涨）。基线还没取回时返回 null，指标条不画这一段：
 * 拿单轮读数顶替会把整条会话说小一个数量级。
 *
 * `lateTurnIds` 是「打开会话时还在跑、因此基线里还没有它们」的那些轮次：它们收口后不会
 * 回填到已经取回的基线里，所以整条都由本地增量承担，读数不会因为收口而回落。
 */
export function sessionCumulative(
  turns: readonly TurnState[],
  baseline: SessionTokenBaseline | null,
  lateTurnIds: ReadonlySet<string> = NO_TURNS,
): SessionCumulative | null {
  if (baseline === null) return null;
  let usage = baseline.usage;
  let elapsedMs = baseline.elapsedMs;
  for (const turn of turns) {
    const covered = baseline.turnIds.has(turn.id) && !lateTurnIds.has(turn.id);
    if (covered) continue;
    usage = addUsage(usage, turn.usage);
    elapsedMs += turn.elapsedMs ?? 0;
  }
  return { ...usage, elapsedMs };
}

/**
 * 累计缓存命中率（万分比）。
 *
 * 分母是完整输入（未缓存 + 缓存读写），分子是缓存读取：与指标条的命中率、浮层里的
 * 「缓存命中」同一个口径，只是这一份按整条会话累计，不是最近一轮。
 */
export function cumulativeCacheHitBasisPoints(usage: SessionUsageTotals): number {
  const denominator = usage.inputTokens + usage.cacheReadTokens + usage.cacheWriteTokens;
  if (denominator <= 0) return 0;
  return Math.min(10_000, Math.floor((usage.cacheReadTokens * 10_000) / denominator));
}
