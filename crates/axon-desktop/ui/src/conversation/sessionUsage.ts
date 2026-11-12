/*
 * 会话级预算读数：上下文与命中率取「最近一次有观测的用量」。
 *
 * 新一轮刚提交、Provider 还没回报用量时，这一轮的用量是空的；直接采用它，指标条就会
 * 在换轮那一刻掉回 0/窗口 与 0%，读者只会读成「数据被清零」。预算读数是会话事实而不是
 * 单轮事实，因此从后往前取第一条有观测的用量；整条会话一次都没观测过时才返回 null，
 * 此时指标条按零读数渲染（新建会话的 `0/窗口` 与 `命中率 0.00%`）。
 */

import { currentContextTokens, totalTokens, type TokenUsage } from './transcript';

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
