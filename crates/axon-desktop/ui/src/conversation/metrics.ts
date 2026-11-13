/*
 * 顶部指标条视图模型。
 *
 * 段序与标签与 TUI 的 HeaderBarViewModel 同源；桌面端省略模型段与工作区路径：
 * 模型由输入坞右下角的选择器常驻承载，工作区已由侧栏与窗口标题说清楚，顶部不再重复。
 *
 * 出现条件按簇分工：预算簇（会话累计、上下文、命中率）从进入会话起常驻，
 * 新会话读作「累计 0」「0/窗口」与「命中率 0.00%」；性能簇与辅助簇缺数据即不渲染该段，
 * 绝不填占位值。
 */

import {
  formatBasisPointsPercent,
  formatDecimalCount,
  formatDurationMillisCompact,
  formatTokensPerSecondX10,
} from './format';
import {
  EMPTY_USAGE,
  currentContextTokens,
  type TokenUsage,
} from './transcript';
import { mcpSegmentText, mcpSegmentTitle, type McpHeaderReading } from './mcpReading';
import { cumulativeCacheHitBasisPoints, type SessionCumulative } from './sessionUsage';
export { currentContextTokens };

/** 上下文压力告警阈值（整数百分比）。 */
const CONTEXT_WARNING_PERCENT = 70;
/** 上下文压力严重阈值（整数百分比）。 */
const CONTEXT_CRITICAL_PERCENT = 85;

/** 段标识。 */
export type HeaderSegmentKind =
  | 'status'
  | 'reasoning'
  | 'sessionTokens'
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
  /** 推理强度。 */
  reasoningEffort: string | null;
  /**
   * 是否已经进入会话（首屏为 false）。
   *
   * 首屏没有会话可言，预算与会话累计读数一律不占位：那里只剩运行态与辅助读数，
   * 一排 `0/1M`、`命中率 0.00%` 只会把「还没开始」说成「已经用光」。
   */
  inSession?: boolean;
  /** 最近一轮的令牌用量；还没跑过任何一轮时为空，上下文段按零读数渲染。 */
  usage: TokenUsage | null;
  /**
   * 整条会话的累计读数（用量分桶与墙钟耗时）；基线还没取回时为空。
   *
   * 口径是各轮输入、缓存读写、输出与推理之和，以及各轮耗时之和，与上下文读数不同：
   * 那个是「最近一次请求占了多少窗口」，这个是「这条会话一共花了多少」。取不到基线时
   * 不画这一段，绝不拿单轮读数顶替。
   */
  sessionCumulative?: SessionCumulative | null;
  /** 当前模型的上下文窗口。 */
  contextWindow: number | null;
  /** 最近一轮首字耗时。 */
  firstTokenAvgMs: number | null;
  /** 最近一轮生成速率，单位 0.1 tok/s。 */
  generationTpsX10: number | null;
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
  // 2 LSP：仅大于 0 时出现。
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
  /*
   * 4 会话读数组：耗时 → 累计 → 命中率 → 上下文。
   *
   * 前三段回答「这条会话一共花了多少」：累计用量是总量，命中率是它里面缓存省下来的比例，
   * 耗时是花掉的时间，因此排在一起、彼此相邻；上下文是「最近一次请求占了多少窗口」，
   * 口径不同，跟在后面不混进这一组。
   *
   * 首屏（还没进入会话）整组不占位：那里没有会话可言，摆一排 `0/1M`、`命中率 0.00%`
   * 反而把「还没开始」说成「已经用光」。
   */
  const cumulative = input.sessionCumulative ?? null;
  const inSession = input.inSession ?? true;
  if (inSession) {
    if (cumulative !== null && cumulative.elapsedMs > 0) {
      segments.push({
        kind: 'elapsed',
        text: shortMetric('耗时', formatDurationMillisCompact(cumulative.elapsedMs)),
        title: `本会话累计耗时 ${formatDurationMillisCompact(cumulative.elapsedMs)}`,
        icon: 'clock',
        tone: 'default',
        breath: false,
      });
    }
    if (cumulative !== null) {
      segments.push({
        kind: 'sessionTokens',
        text: shortMetric('累计', formatDecimalCount(cumulative.totalTokens)),
        title: `本会话累计消耗 ${formatDecimalCount(cumulative.totalTokens)} token`,
        tone: 'default',
        breath: false,
      });
      // 命中率按整条会话累计：与累计用量同一份基线，不是最近一轮的命中率。
      segments.push({
        kind: 'cacheHit',
        text: shortMetric(
          '命中率',
          formatBasisPointsPercent(cumulativeCacheHitBasisPoints(cumulative)),
        ),
        title: '本会话累计缓存命中率',
        tone: 'default',
        breath: false,
      });
    }
    // 上下文：进入会话即常驻，没有观测时读作 0/窗口。
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
  }
  // 5 计划：有步骤时才出现。
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
  // 9 耗时已并入上面的会话累计：这一行不再重复报单轮耗时。
  return segments;
}

/** 指标条的四簇：身份、会话读数、性能、辅助。 */
export type HeaderClusterKind = 'identity' | 'session' | 'performance' | 'aux';

/** 指标条的一段成簇结果。 */
export interface HeaderCluster {
  kind: HeaderClusterKind;
  segments: HeaderSegment[];
}

/** 段到簇的归属；不在表里的段不出现。 */
const CLUSTER_OF: Record<HeaderSegmentKind, HeaderClusterKind> = {
  status: 'identity',
  reasoning: 'identity',
  sessionTokens: 'session',
  elapsed: 'session',
  cacheHit: 'session',
  context: 'session',
  plan: 'aux',
  lsp: 'aux',
  mcp: 'aux',
  skill: 'aux',
  firstToken: 'performance',
  generationRate: 'performance',
};

/**
 * 把平铺的段按语义分成四簇。
 *
 * 四簇的关系是「在不在干活 · 这条会话花了多少 · 跑得多快 · 辅助状态」，
 * 因此装不下时整簇收起，而不是从中间把某一簇切成两半。
 * 工作区路径不进指标条：侧栏与窗口标题已经说清楚当前在哪，顶部再写一遍只是重复。
 * LSP、MCP、技能与计划是辅助读数，固定排在最后，窗口不够时先让位。
 */
export function headerClusters(input: HeaderMetricsInput): HeaderCluster[] {
  const groups: HeaderCluster[] = [
    { kind: 'identity', segments: [] },
    { kind: 'session', segments: [] },
    { kind: 'performance', segments: [] },
    { kind: 'aux', segments: [] },
  ];
  for (const segment of headerSegments(input)) {
    groups.find((group) => group.kind === CLUSTER_OF[segment.kind])?.segments.push(segment);
  }
  return groups.filter((group) => group.segments.length > 0);
}
