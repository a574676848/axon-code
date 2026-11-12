/*
 * 顶部指标条视图模型。
 *
 * 段序与标签与 TUI 的 HeaderBarViewModel 同源；桌面端省略模型段：
 * 模型由输入坞右下角的选择器常驻承载，顶部不再重复。
 *
 * 出现条件按簇分工：预算簇（上下文、命中率）从进入会话起常驻，
 * 新会话读作「0/窗口」与「命中率 0.00%」；性能簇与辅助簇缺数据即不渲染该段，
 * 绝不填占位值。
 */

import {
  ellipsize,
  formatBasisPointsPercent,
  formatDecimalCount,
  formatDurationMillisCompact,
  formatTokensPerSecondX10,
} from './format';
import {
  EMPTY_USAGE,
  cacheHitRateBasisPoints,
  currentContextTokens,
  type TokenUsage,
} from './transcript';
import { mcpSegmentText, mcpSegmentTitle, type McpHeaderReading } from './mcpReading';
export { currentContextTokens };

/** 工作目录显示宽度预算，含省略号。 */
const CWD_VISIBLE_CHARS = 24;
/** 上下文压力告警阈值（整数百分比）。 */
const CONTEXT_WARNING_PERCENT = 70;
/** 上下文压力严重阈值（整数百分比）。 */
const CONTEXT_CRITICAL_PERCENT = 85;

/** 段标识。 */
export type HeaderSegmentKind =
  | 'status'
  | 'cwd'
  | 'reasoning'
  | 'context'
  | 'cacheHit'
  | 'firstToken'
  | 'generationRate'
  | 'elapsed'
  | 'plan'
  | 'lsp'
  | 'mcp'
  | 'skill';

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
  /** 原生提示全文；缺省时渲染层用 `text`。 */
  title?: string;
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
  /** 最近一轮的令牌用量；还没跑过任何一轮时为空，预算簇按零读数渲染。 */
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
  /** MCP 运行态；没有服务器且没有错误时为空，指标条不画这一段。 */
  mcp?: McpHeaderReading | null;
  /** 已安装技能条数；未读到清单时为空，零条时指标条不画这一段。 */
  skillCount?: number | null;
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
 * 缓存读取与写入仍然收进 ContextUsagePopover；命中率是读者会直接看的预算指标，
 * 在下一段直接给出，不再藏进浮层。已用为零时同样给出读数（`0/窗口`），
 * 因为「这一轮还没花预算」本身就是要看的事实。
 */
function contextText(usage: TokenUsage, contextWindow: number | null): string {
  const used = currentContextTokens(usage);
  if (contextWindow !== null && contextWindow > 0) {
    return `${formatDecimalCount(used)}/${formatDecimalCount(contextWindow)}`;
  }
  return formatDecimalCount(used);
}

/** 组装指标条的各段。 */
export function headerSegments(input: HeaderMetricsInput): HeaderSegment[] {
  const segments: HeaderSegment[] = [];
  /*
   * 预算簇的读数基准：没有用量观测就是零用量。
   *
   * 新建会话、刚进入会话都还没有任何一轮的观测，此时预算不是「未知」而是「还没用」，
   * 因此它照样参与渲染，不能靠隐藏这一段来表达未知。
   */
  const usage = input.usage ?? EMPTY_USAGE;

  // 1 状态：仅工作态出现。品牌名不再占位：工作态只留一枚呼吸的活动点，
  //   生成到哪一步已经由正文与输入坞那一行说清楚了。
  if (input.working) {
    segments.push({
      kind: 'status',
      text: '',
      title: '正在运行',
      icon: 'spark',
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
  // 3 LSP：仅大于 0 时出现。
  if (input.lspActiveCount > 0) {
    segments.push({
      kind: 'lsp',
      text: shortMetric('LSP', String(input.lspActiveCount)),
      icon: 'code',
      tone: 'default',
      breath: false,
    });
  }
  // 3.1 MCP：有服务器或读不到结论时出现。异常一台都不许沉默，用危险色顶出来；
  // 正常时只报台数，与 LSP 同为辅助读数。
  if (input.mcp !== null && input.mcp !== undefined) {
    segments.push({
      kind: 'mcp',
      text: mcpSegmentText(input.mcp),
      title: mcpSegmentTitle(input.mcp),
      icon: 'mcp',
      tone: input.mcp.failedCount > 0 || input.mcp.error !== null ? 'critical' : 'default',
      breath: false,
    });
  }
  // 3.2 技能：紧跟 MCP 报「装了多少条」。清单未读到或零条时不占位，
  // 与 LSP、MCP 同一口径：辅助读数只报事实，不拿 0 占宽度。
  if (input.skillCount !== null && input.skillCount !== undefined && input.skillCount > 0) {
    segments.push({
      kind: 'skill',
      text: shortMetric('技能', String(input.skillCount)),
      title: `共 ${input.skillCount} 项已安装技能`,
      icon: 'skill',
      tone: 'default',
      breath: false,
    });
  }
  // 4 上下文：预算簇从进入会话起常驻，没有观测时读作 0/窗口。
  segments.push({
    kind: 'context',
    text: contextText(usage, input.contextWindow),
    tone:
      input.contextWindow !== null && input.contextWindow > 0
        ? contextPressureTone(currentContextTokens(usage), input.contextWindow)
        : 'default',
    breath: false,
    icon: 'ring',
  });
  // 5 缓存命中率：与上下文段同一生命周期，没有完整输入 token 时命中率就是 0%。
  //   藏起来会让新会话的指标条凭空少一段，读者只能猜是没命中还是没测到。
  segments.push({
    kind: 'cacheHit',
    text: shortMetric('命中率', formatBasisPointsPercent(cacheHitRateBasisPoints(usage))),
    tone: 'default',
    breath: false,
  });
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

/** 指标条的四簇：身份、预算、性能、辅助。 */
export type HeaderClusterKind = 'identity' | 'budget' | 'performance' | 'aux';

/** 指标条的一段成簇结果。 */
export interface HeaderCluster {
  kind: HeaderClusterKind;
  segments: HeaderSegment[];
}

/** 段到簇的归属；不在表里的段不出现。 */
const CLUSTER_OF: Record<HeaderSegmentKind, HeaderClusterKind> = {
  status: 'identity',
  cwd: 'identity',
  reasoning: 'identity',
  context: 'budget',
  cacheHit: 'budget',
  plan: 'aux',
  lsp: 'aux',
  mcp: 'aux',
  skill: 'aux',
  firstToken: 'performance',
  generationRate: 'performance',
  elapsed: 'performance',
};

/**
 * 把平铺的段按语义分成四簇。
 *
 * 四簇的关系是「谁在哪干活 · 还剩多少预算 · 跑得多快 · 辅助状态」，
 * 因此装不下时整簇收起，而不是从中间把某一簇切成两半。
 * LSP、MCP、技能与计划是辅助读数，固定排在最后，窗口不够时先让位。
 */
export function headerClusters(input: HeaderMetricsInput): HeaderCluster[] {
  const groups: HeaderCluster[] = [
    { kind: 'identity', segments: [] },
    { kind: 'budget', segments: [] },
    { kind: 'performance', segments: [] },
    { kind: 'aux', segments: [] },
  ];
  for (const segment of headerSegments(input)) {
    groups.find((group) => group.kind === CLUSTER_OF[segment.kind])?.segments.push(segment);
  }
  return groups.filter((group) => group.segments.length > 0);
}
