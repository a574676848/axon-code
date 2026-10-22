/*
 * 顶部指标条视图模型。
 *
 * 与 TUI 的 HeaderBarViewModel::from_sources 逐段恒等：段序、标签、出现条件、
 * 截断规则与色调阈值全部一致。缺数据即不渲染该段，绝不填占位值。
 */

import {
  ellipsize,
  formatDecimalCount,
  formatDurationMillisCompact,
  formatTokensPerSecondX10,
  modelNameOnly,
} from './format';
import { currentContextTokens, type TokenUsage } from './transcript';
export { currentContextTokens };

/** 模型名显示宽度预算，含省略号。 */
const MODEL_VISIBLE_CHARS = 20;
/** 工作目录显示宽度预算，含省略号。 */
const CWD_VISIBLE_CHARS = 24;
/** 上下文压力告警阈值（整数百分比）。 */
const CONTEXT_WARNING_PERCENT = 70;
/** 上下文压力严重阈值（整数百分比）。 */
const CONTEXT_CRITICAL_PERCENT = 85;
/** 工作状态指示文本。图标由渲染层给，文本里不再带 emoji。 */
export const ACTIVITY_LABEL = 'Axon';

/** 段标识。 */
export type HeaderSegmentKind =
  | 'status'
  | 'model'
  | 'cwd'
  | 'reasoning'
  | 'context'
  | 'firstToken'
  | 'generationRate'
  | 'elapsed'
  | 'plan'
  | 'lsp';

/** 段色调。 */
export type HeaderSegmentTone = 'default' | 'warning' | 'critical';

/** 指标条上的一段。 */
export interface HeaderSegment {
  /** 段类型，供渲染层选择样式。 */
  kind: HeaderSegmentKind;
  /** 展示文本。 */
  text: string;
  /** 语义色调。 */
  tone: HeaderSegmentTone;
  /** 是否需要呼吸高亮。 */
  breath: boolean;
  /** 指标图标名；无图标时省略。 */
  icon?: string;
}

/** 指标条输入。 */
export interface HeaderMetricsInput {
  /** 是否正在工作。 */
  working: boolean;
  /** 呼吸高亮的相位，工作态下用于脉冲。 */
  animationTick: number;
  /** 当前模型，允许带 provider 前缀。 */
  model: string;
  /** 工作目录。 */
  cwd: string;
  /** 推理强度。 */
  reasoningEffort: string | null;
  /** 最近一轮的令牌用量。 */
  usage: TokenUsage | null;
  /** 当前模型的上下文窗口。 */
  contextWindow: number | null;
  /** 最近一轮首字耗时。 */
  firstTokenAvgMs: number | null;
  /** 最近一轮生成速率，单位 0.1 tok/s。 */
  generationTpsX10: number | null;
  /** 最近一轮耗时；仅在工作结束时有值。 */
  elapsedMs: number | null;
  /** 计划进度。 */
  plan: { completed: number; total: number } | null;
  /** 活跃 LSP 客户端数。 */
  lspActiveCount: number;
}

function shortMetric(label: string, value: string): string {
  // 与轮次分割线同一写法：标签与读数之间只隔一个空格，不加冒号。
  return `${label} ${value}`;
}

function contextPressureTone(used: number, window: number): HeaderSegmentTone {
  const percent = window <= 0 ? 0 : Math.floor((used * 100) / window);
  if (percent >= CONTEXT_CRITICAL_PERCENT) return 'critical';
  if (percent >= CONTEXT_WARNING_PERCENT) return 'warning';
  return 'default';
}

/**
 * 上下文段文本：只给「已用/窗口」。
 *
 * 缓存读数（读取、写入、命中率）是展开后才会关心的明细，收进 ContextUsagePopover；
 * 顶部这一行因此不再出现「缓存 12K 命中 44.75% 写入 0」这种一串数字。
 */
function contextText(usage: TokenUsage, contextWindow: number | null): string | null {
  const used = currentContextTokens(usage);
  if (used === 0) return null;
  if (contextWindow !== null && contextWindow > 0) {
    return `${formatDecimalCount(used)}/${formatDecimalCount(contextWindow)}`;
  }
  return formatDecimalCount(used);
}

/** 组装指标条的十段。 */
export function headerSegments(input: HeaderMetricsInput): HeaderSegment[] {
  const segments: HeaderSegment[] = [];

  // 1 状态：仅工作态出现。
  if (input.working) {
    segments.push({
      kind: 'status',
      text: ACTIVITY_LABEL,
      tone: 'default',
      breath: input.animationTick % 4 < 2,
    });
  }
  // 2 工作目录：恒出现，无标签，按显示宽度截断。
  segments.push({
    kind: 'cwd',
    text: ellipsize(input.cwd, CWD_VISIBLE_CHARS),
    tone: 'default',
    breath: false,
    icon: 'folder',
  });
  // 3 模型：与目录同属「这是谁在哪干活」的身份信息。
  const model = ellipsize(modelNameOnly(input.model), MODEL_VISIBLE_CHARS);
  if (model.length > 0) {
    segments.push({
      kind: 'model',
      text: model,
      tone: 'default',
      breath: false,
    });
  }
  // 4 LSP：仅大于 0 时出现。
  if (input.lspActiveCount > 0) {
    segments.push({
      kind: 'lsp',
      text: shortMetric('LSP', String(input.lspActiveCount)),
      icon: 'code',
      tone: 'default',
      breath: false,
    });
  }
  // 5 上下文：无用量观测时不出现。
  if (input.usage !== null) {
    const text = contextText(input.usage, input.contextWindow);
    if (text !== null) {
      const used = currentContextTokens(input.usage);
      segments.push({
        kind: 'context',
        text,
        tone:
          input.contextWindow !== null && input.contextWindow > 0
            ? contextPressureTone(used, input.contextWindow)
            : 'default',
        breath: false,
        icon: 'ring',
      });
    }
  }
  // 6 计划：有步骤时才出现。
  if (input.plan !== null && input.plan.total > 0) {
    segments.push({
      kind: 'plan',
      text: shortMetric('计划', `${input.plan.completed}/${input.plan.total}`),
      icon: 'list',
      tone: 'default',
      breath: false,
    });
  }
  // 7 首字。
  if (input.firstTokenAvgMs !== null) {
    segments.push({
      kind: 'firstToken',
      text: shortMetric('首字', formatDurationMillisCompact(input.firstTokenAvgMs)),
      icon: 'clock',
      tone: 'default',
      breath: false,
    });
  }
  // 8 速率。
  if (input.generationTpsX10 !== null) {
    segments.push({
      kind: 'generationRate',
      text: shortMetric('速率', formatTokensPerSecondX10(input.generationTpsX10)),
      icon: 'gauge',
      tone: 'default',
      breath: false,
    });
  }
  // 9 耗时：只在工作结束后出现，与状态段互补。
  if (!input.working && input.elapsedMs !== null && input.elapsedMs > 0) {
    segments.push({
      kind: 'elapsed',
      text: shortMetric('耗时', formatDurationMillisCompact(input.elapsedMs)),
      icon: 'clock',
      tone: 'default',
      breath: false,
    });
  }
  return segments;
}

/** 指标条的三簇：身份、预算、性能。 */
export type HeaderClusterKind = 'identity' | 'budget' | 'performance';

/** 指标条的一段成簇结果。 */
export interface HeaderCluster {
  kind: HeaderClusterKind;
  segments: HeaderSegment[];
}

/** 段到簇的归属；不在表里的段不出现。 */
const CLUSTER_OF: Record<HeaderSegmentKind, HeaderClusterKind> = {
  status: 'identity',
  cwd: 'identity',
  model: 'identity',
  reasoning: 'identity',
  lsp: 'identity',
  context: 'budget',
  plan: 'budget',
  firstToken: 'performance',
  generationRate: 'performance',
  elapsed: 'performance',
};

/**
 * 把平铺的段按语义分成三簇。
 *
 * 三簇的关系是「谁在哪干活 · 还剩多少预算 · 跑得多快」，
 * 因此装不下时整簇收起，而不是从中间把某一簇切成两半。
 */
export function headerClusters(input: HeaderMetricsInput): HeaderCluster[] {
  const groups: HeaderCluster[] = [
    { kind: 'identity', segments: [] },
    { kind: 'budget', segments: [] },
    { kind: 'performance', segments: [] },
  ];
  for (const segment of headerSegments(input)) {
    groups.find((group) => group.kind === CLUSTER_OF[segment.kind])?.segments.push(segment);
  }
  return groups.filter((group) => group.segments.length > 0);
}
