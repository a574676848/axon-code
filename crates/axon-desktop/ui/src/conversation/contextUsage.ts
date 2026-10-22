/*
 * 上下文使用弹窗的视图模型。
 *
 * 只做一件事：把「Provider 报出的上下文总量」与「本地估算的构成」摆到同一张卡上。
 * 两者口径天然不同（Provider 用自己的分词器，本地按 Provider 可见字节数折算），
 * 因此这里不把估算硬凑成总量：总量用实测值，构成用估算值，差额单独成段讲清楚。
 * 缺观测就不画：宁可不出现，也不填 0 或占位值。
 */

import { formatBasisPointsPercent, formatDecimalCount } from './format';
import {
  cacheHitRateBasisPoints,
  compositionTokens,
  currentContextTokens,
  type TokenUsage,
} from './transcript';

/** 构成分段类型。 */
export type ContextUsageSegmentKind = 'systemPrompt' | 'toolDefinitions' | 'conversation' | 'other';

/** 构成分段。 */
export interface ContextUsageSegment {
  kind: ContextUsageSegmentKind;
  /** 展示标签。 */
  label: string;
  tokens: number;
  /** 是否为本地估算（估算值展示时带 `~`）。 */
  estimated: boolean;
  /** 该段为何是估算的补充说明，作为 title 提示。 */
  hint: string;
  /** 分段与图例共用的色块类：同一蓝色梯度，差额段走中性灰。 */
  swatchClassName: string;
  /** 色带上的宽度百分比（相对上下文窗口，累加不超过 100）。 */
  barPercent: number;
}

/** 上下文使用弹窗模型。 */
export interface ContextUsageModel {
  /** Provider 报出的当前上下文 token。 */
  usedTokens: number;
  /** 当前模型的上下文窗口；未知时为 null。 */
  windowTokens: number | null;
  /** 已用百分比；无窗口时为 null。 */
  usedPercent: number | null;
  /** 总量文本：有窗口时 `122K / 1M`，否则只有已用。 */
  windowText: string;
  /** 构成分段；没有构成观测时为空数组。 */
  segments: ContextUsageSegment[];
  /** 是否有构成观测；无观测时界面只显示总量并显式说明。 */
  hasComposition: boolean;
  /**
   * 缓存明细。
   *
   * 顶部指标条只保留「已用/窗口」，读取、写入与命中率收到这里：
   * 它们是同一个预算问题的细节，不该在指标条上排成一串数字。
   */
  cacheRows: ContextUsageRow[];
}

/** 弹窗里的一行附加读数。 */
export interface ContextUsageRow {
  label: string;
  value: string;
}

/** 系统提示词的色块。 */
const SWATCH_SYSTEM = 'bg-text-link/25';
/** 工具定义的色块。 */
const SWATCH_TOOLS = 'bg-text-link/55';
/** 对话消息的色块。 */
const SWATCH_CONVERSATION = 'bg-text-link';
/** 口径差额的色块：中性灰，明示它不是估算的一部分。 */
const SWATCH_OTHER = 'bg-state-idle';

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
      swatch: SWATCH_SYSTEM,
      hint: '本地估算：Axon 注入的系统提示词、工作区约定与上下文片段',
    });
    pushSegment(segments, {
      kind: 'toolDefinitions',
      label: '工具定义',
      tokens: usage.toolDefinitionTokens,
      swatch: SWATCH_TOOLS,
      hint: '本地估算：本次请求携带的工具 schema',
    });
    pushSegment(segments, {
      kind: 'conversation',
      label: '对话消息',
      tokens: usage.conversationTokens,
      swatch: SWATCH_CONVERSATION,
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
        swatch: SWATCH_OTHER,
        hint: 'Provider 实测总量与本地估算的差额：分词口径与协议开销',
      });
    }
    assignBarPercents(segments, usedTokens, windowTokens);
  }
  return {
    usedTokens,
    windowTokens,
    usedPercent: percentOf(usedTokens, windowTokens),
    windowText:
      windowTokens === null
        ? formatDecimalCount(usedTokens)
        : `${formatDecimalCount(usedTokens)} / ${formatDecimalCount(windowTokens)}`,
    segments,
    hasComposition: estimatedTokens > 0,
    cacheRows: cacheUsageRows(usage),
  };
}

/**
 * 缓存明细：只列有观测的项。
 *
 * 命中率恒有定义（分母就是输入总量），因此它总会出现；
 * 读取与写入为零时不占行，避免「写入 0」这种噪声。
 */
function cacheUsageRows(usage: TokenUsage): ContextUsageRow[] {
  const rows: ContextUsageRow[] = [];
  if (usage.cacheReadInputTokens > 0) {
    rows.push({ label: '缓存读取', value: formatDecimalCount(usage.cacheReadInputTokens) });
  }
  if (usage.cacheWriteInputTokens > 0) {
    rows.push({ label: '缓存写入', value: formatDecimalCount(usage.cacheWriteInputTokens) });
  }
  rows.push({
    label: '缓存命中率',
    value: formatBasisPointsPercent(cacheHitRateBasisPoints(usage)),
  });
  return rows;
}

interface SegmentInput {
  kind: ContextUsageSegmentKind;
  label: string;
  tokens: number;
  swatch: string;
  hint: string;
}

/** 差额至少占标尺一个整数百分点才单独成段。 */
function isVisibleGap(gap: number, base: number): boolean {
  return base > 0 && Math.floor((gap * 100) / base) >= 1;
}

/** 只推进非零分段：没有观测的类别不占行也不占色带。 */
function pushSegment(target: ContextUsageSegment[], input: SegmentInput): void {
  if (input.tokens <= 0) return;
  target.push({
    kind: input.kind,
    label: input.label,
    tokens: input.tokens,
    estimated: input.kind !== 'other',
    hint: input.hint,
    swatchClassName: input.swatch,
    barPercent: 0,
  });
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
