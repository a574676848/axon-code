/*
 * 上下文使用弹窗的视图模型。
 *
 * 只做一件事：把「Provider 报出的上下文总量」与「本地估算的构成」摆到同一张卡上。
 * 两者口径天然不同（Provider 用自己的分词器，本地按 Provider 可见字节数折算），
 * 因此这里不把估算硬凑成总量：总量用实测值，构成用估算值，差额单独成段讲清楚。
 * 缺观测就不画：宁可不出现，也不填 0 或占位值。
 */

import { formatDecimalCount } from './format';
import { compositionTokens, currentContextTokens, type TokenUsage } from './transcript';

/** 构成分段类型。 */
export type ContextUsageSegmentKind = 'systemPrompt' | 'toolDefinitions' | 'conversation' | 'other';

/** 构成分段。 */
export interface ContextUsageSegment {
  /** 稳定标识。 */
  kind: ContextUsageSegmentKind;
  /** 展示标签。 */
  label: string;
  /** 该段的 token 数。 */
  tokens: number;
  /** 是否为本地估算（估算值展示时带 `~`）。 */
  estimated: boolean;
  /** 该段为何是估算的补充说明。 */
  hint: string;
  /** 色带与色块共用的颜色（主题令牌）。 */
  color: string;
  /** 色带上的宽度百分比（相对标尺，累加不超过 100）。 */
  barPercent: number;
}

/** 上下文使用弹窗模型。 */
export interface ContextUsageModel {
  /** Provider 报出的当前上下文 token。 */
  usedTokens: number;
  /** 已用比例（0~1），给标题前的环形图形用。 */
  usedRatio: number;
  /** 标题里的强调值（如 `63%`）；无窗口时为空。 */
  percentText: string | null;
  /** 总量文本：有窗口时 `122K / 1M`，否则只有已用。 */
  totalText: string;
  /** 构成分段；没有构成观测时为空数组，卡片显式降级。 */
  segments: ContextUsageSegment[];
}

/** 系统提示词的色块：同一强调色的浅档。 */
const COLOR_SYSTEM = 'color-mix(in srgb, var(--color-text-link) 25%, transparent)';
/** 工具定义的色块：中档。 */
const COLOR_TOOLS = 'color-mix(in srgb, var(--color-text-link) 55%, transparent)';
/** 对话消息的色块：实色。 */
const COLOR_CONVERSATION = 'var(--color-text-link)';
/** 口径差额的色块：中性灰，明示它不是估算的一部分。 */
const COLOR_OTHER = 'var(--color-state-idle)';

/** 百分比取整；上限 100。 */
function percentOf(tokens: number, base: number | null): number | null {
  if (base === null || base <= 0) return null;
  return Math.min(100, Math.floor((tokens * 100) / base));
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
  const segments: ContextUsageSegment[] = [];
  if (estimatedTokens > 0) {
    pushSegment(segments, {
      kind: 'systemPrompt',
      label: '系统提示词',
      tokens: usage.systemPromptTokens,
      color: COLOR_SYSTEM,
      hint: '本地估算：Axon 注入的系统提示词、工作区约定与上下文片段',
    });
    pushSegment(segments, {
      kind: 'toolDefinitions',
      label: '工具定义',
      tokens: usage.toolDefinitionTokens,
      color: COLOR_TOOLS,
      hint: '本地估算：本次请求携带的工具 schema',
    });
    pushSegment(segments, {
      kind: 'conversation',
      label: '对话消息',
      tokens: usage.conversationTokens,
      color: COLOR_CONVERSATION,
      hint: '本地估算：用户、助手与工具回执消息',
    });
    // 实测总量大于估算总量时，差额单独成段，否则色带会对不上标题里的实测值。
    // 零头不单独成行：几十个 token 的口径差异占不了一格，列出来只是噪声。
    const gap = usedTokens - estimatedTokens;
    if (gap > 0 && isVisibleGap(gap, windowTokens ?? usedTokens)) {
      pushSegment(segments, {
        kind: 'other',
        label: '其他',
        tokens: gap,
        color: COLOR_OTHER,
        hint: 'Provider 实测总量与本地估算的差额：分词口径与协议开销',
      });
    }
    assignBarPercents(segments, usedTokens, windowTokens);
  }
  const percent = percentOf(usedTokens, windowTokens);
  return {
    usedTokens,
    usedRatio: windowTokens === null ? 1 : Math.min(1, usedTokens / windowTokens),
    percentText: percent === null ? null : `${percent}%`,
    totalText:
      windowTokens === null
        ? formatDecimalCount(usedTokens)
        : `${formatDecimalCount(usedTokens)} / ${formatDecimalCount(windowTokens)}`,
    segments,
  };
}

/** 只推进非零分段：没有观测的类别不占行也不占色带。 */
function pushSegment(
  target: ContextUsageSegment[],
  input: {
    kind: ContextUsageSegmentKind;
    label: string;
    tokens: number;
    color: string;
    hint: string;
  },
): void {
  if (input.tokens <= 0) return;
  target.push({
    kind: input.kind,
    label: input.label,
    tokens: input.tokens,
    estimated: input.kind !== 'other',
    hint: input.hint,
    color: input.color,
    barPercent: 0,
  });
}

/** 差额至少占标尺一个整数百分点才单独成段。 */
function isVisibleGap(gap: number, base: number): boolean {
  return base > 0 && Math.floor((gap * 100) / base) >= 1;
}

/**
 * 分配色带宽度。
 *
 * 标尺优先取上下文窗口；窗口未知时取实测总量，此时色带恰好铺满。
 * 逐段向下取整会把零头丢成可见的空隙，因此把余量补给最大的一段：
 * 色带的彩色部分于是恒等于标题里的已用比例，两处读数不会互相打脸。
 */
function assignBarPercents(
  segments: ContextUsageSegment[],
  usedTokens: number,
  windowTokens: number | null,
): void {
  const base = windowTokens ?? usedTokens;
  if (base <= 0) return;
  const target = percentOf(usedTokens, base) ?? 0;
  let assigned = 0;
  for (const segment of segments) {
    segment.barPercent = Math.min(100 - assigned, Math.floor((segment.tokens * 100) / base));
    assigned += segment.barPercent;
  }
  const leftover = target - assigned;
  if (leftover <= 0) return;
  const widest = segments.reduce((best, segment) =>
    segment.tokens > best.tokens ? segment : best,
  );
  widest.barPercent += leftover;
}

/** 分段数值文本：估算加 `~`，差额不加。 */
export function segmentValueText(segment: ContextUsageSegment): string {
  const value = formatDecimalCount(segment.tokens);
  return segment.estimated ? `~${value}` : value;
}
