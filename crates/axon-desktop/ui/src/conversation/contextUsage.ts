/*
 * 上下文使用弹窗的视图模型。
 *
 * 只做一件事：把「Provider 报出的上下文总量」与「本地估算的构成」摆到同一张卡上。
 * 两者口径天然不同（Provider 用自己的分词器，本地按 Provider 可见字节数折算），
 * 因此这里不把估算硬凑成总量：总量用实测值，构成用估算值，差额单独成行讲清楚。
 * 缺观测就不画：宁可不出现，也不填 0 或占位值。
 */

import { formatDecimalCount } from './format';
import { compositionTokens, currentContextTokens, type TokenUsage } from './transcript';

/** 弹窗里的一行读数。 */
export interface ContextUsageRow {
  /** 行标签。 */
  label: string;
  /** 行数值（估算值带 `~`）。 */
  value: string;
}

/** 上下文使用弹窗模型。 */
export interface ContextUsageModel {
  /** Provider 报出的当前上下文 token。 */
  usedTokens: number;
  /** 已用比例（0~1），给标题前的环形图形用。 */
  usedRatio: number;
  /** 总量文本：有窗口时 `122K / 1M`，否则只有已用。 */
  totalText: string;
  /** 构成明细；没有构成观测时为空数组，卡片显式降级。 */
  rows: ContextUsageRow[];
}

/**
 * 组装弹窗模型。
 *
 * 返回 null 表示这一轮根本没有上下文观测，此时连入口都不该出现。
 */
export function contextUsageModel(
  usage: TokenUsage | null,
  contextWindow: number | null,
): ContextUsageModel | null {
  if (usage === null) return null;
  const usedTokens = currentContextTokens(usage);
  if (usedTokens === 0) return null;
  const windowTokens = contextWindow !== null && contextWindow > 0 ? contextWindow : null;
  const estimatedTokens = compositionTokens(usage);
  const rows: ContextUsageRow[] = [];
  if (estimatedTokens > 0) {
    pushRow(rows, '系统提示词', usage.systemPromptTokens, true);
    pushRow(rows, '工具定义', usage.toolDefinitionTokens, true);
    pushRow(rows, '对话消息', usage.conversationTokens, true);
    // 实测总量大于估算总量时，差额单独成行：否则各行之和对不上标题里的实测值。
    // 零头不单独成行：几十个 token 的口径差异列出来只是噪声。
    const gap = usedTokens - estimatedTokens;
    if (gap > 0 && isVisibleGap(gap, windowTokens ?? usedTokens)) {
      pushRow(rows, '其他', gap, false);
    }
  }
  return {
    usedTokens,
    usedRatio: windowTokens === null ? 1 : Math.min(1, usedTokens / windowTokens),
    totalText:
      windowTokens === null
        ? formatDecimalCount(usedTokens)
        : `${formatDecimalCount(usedTokens)} / ${formatDecimalCount(windowTokens)}`,
    rows,
  };
}

/** 只推进非零构成：没有观测的类别不占行。 */
function pushRow(
  target: ContextUsageRow[],
  label: string,
  tokens: number,
  estimated: boolean,
): void {
  if (tokens <= 0) return;
  const value = formatDecimalCount(tokens);
  target.push({ label, value: estimated ? `~${value}` : value });
}

/** 差额至少占标尺一个整数百分点才单独成行。 */
function isVisibleGap(gap: number, base: number): boolean {
  return base > 0 && Math.floor((gap * 100) / base) >= 1;
}
