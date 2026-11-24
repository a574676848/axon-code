/*
 * 顶部指标条视图模型。
 *
 * 段序与标签与 TUI 的 HeaderBarViewModel 同源；桌面端省略模型段与工作区路径：
 * 模型由输入坞右下角的选择器常驻承载，工作区已由侧栏与窗口标题说清楚，顶部不再重复。
 *
 * 出现条件按簇分工：预算簇（会话累计、上下文、命中率）从进入会话起常驻，
 * 新会话读作「累计 0」「0/窗口」与「命中率 0.00%」；性能簇与辅助簇缺数据即不渲染该段，
 * 绝不填占位值。
 *
 * 上下文段与剩余段只读会话上下文快照（见 `contextSnapshot.ts`）：两者同一份带时间戳
 * 的事实，未知就显示未知，不拿用量帧或模型目录的窗口值顶替。
 */

import {
  formatBasisPointsPercent,
  formatDecimalCount,
  formatDurationMillisCompact,
  formatTokensPerSecondX10,
} from './format';
import {
  contextPairText,
  type ContextSnapshotReading,
} from './contextSnapshot';
import { cumulativeCacheHitBasisPoints, type SessionCumulative } from './sessionUsage';

/** 上下文压力告警阈值（整数百分比）。 */
const CONTEXT_WARNING_PERCENT = 70;
/** 上下文压力严重阈值（整数百分比）。 */
const CONTEXT_CRITICAL_PERCENT = 85;

/** 段标识。 */
export type HeaderSegmentKind =
  | 'reasoning'
  | 'sessionTokens'
  | 'context'
  | 'remaining'
  | 'cacheHit'
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
  /** 指标图标名；无图标时省略。 */
  icon?: string;
  /** 原生提示全文；缺省时渲染层用 `text`。 */
  title?: string;
}

/** 指标条输入。 */
export interface HeaderMetricsInput {
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
  /**
   * 会话上下文快照读数：占用、窗口与换窗数字都取这一份。
   *
   * 快照为空（还没有任何可用读数）时上下文段整段不占位；快照里缺的字段按未知显示，
   * 既不沿用上一份读数，也不拿模型目录里的窗口值补分母。
   */
  contextSnapshot: ContextSnapshotReading | null;
  /**
   * 整条会话的累计读数（用量分桶与墙钟耗时）；基线还没取回时为空。
   *
   * 口径是各轮输入、缓存读写、输出与推理之和，以及各轮耗时之和，与上下文读数不同：
   * 那个是「最近一次请求占了多少窗口」，这个是「这条会话一共花了多少」。取不到基线时
   * 不画这一段，绝不拿单轮读数顶替。
   */
  sessionCumulative?: SessionCumulative | null;
  /** 最近一轮首字耗时。 */
  firstTokenAvgMs: number | null;
  /** 最近一轮生成速率，单位 0.1 tok/s。 */
  generationTpsX10: number | null;
  /** 计划进度。 */
  plan: { completed: number; total: number } | null;
  /** 活跃 LSP 客户端数。 */
  lspActiveCount: number;
}

function shortMetric(label: string, value: string): string {
  // 与轮次分割线同一写法：标签与读数之间只隔一个空格，不加冒号。
  return `${label} ${value}`;
}

/**
 * 上下文段色调：按已用占窗口的比例分档。
 *
 * 分母未知时不给色调：没有窗口就谈不上「快满了」，涂成告警色只会制造焦虑。
 */
function contextPressureTone(used: number, window: number | null): HeaderSegmentTone {
  if (window === null || window <= 0) return 'default';
  const percent = Math.floor((used * 100) / window);
  if (percent >= CONTEXT_CRITICAL_PERCENT) return 'critical';
  if (percent >= CONTEXT_WARNING_PERCENT) return 'warning';
  return 'default';
}

/** 组装指标条的各段。 */
export function headerSegments(input: HeaderMetricsInput): HeaderSegment[] {
  const segments: HeaderSegment[] = [];
  // 1 LSP：仅大于 0 时出现。
  if (input.lspActiveCount > 0) {
    segments.push({
      kind: 'lsp',
      text: shortMetric('LSP', String(input.lspActiveCount)),
      icon: 'lsp',
      tone: 'default',
        });
  }
  /*
   * 3 会话读数组：耗时 → 累计 → 命中率 → 上下文。
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
            });
    }
    // 累计与命中率同上下文一个生命周期：还没有任何用量观测时整段不占位。
    // 摆一个 `累计 0` 与 `命中率 0.00%` 只是把「还没开始」说成「已经算过」。
    if (cumulative !== null && cumulative.totalTokens > 0) {
      segments.push({
        kind: 'sessionTokens',
        text: shortMetric('累计', formatDecimalCount(cumulative.totalTokens)),
        title: `本会话累计消耗 ${formatDecimalCount(cumulative.totalTokens)} token`,
        icon: 'database',
        tone: 'default',
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
            });
    }
    /*
     * 上下文：占用与窗口分母都取同一份快照。
     *
     * 还没有任何读数时整段不占位；有读数之后常驻，分母未知就写「未知」，绝不写 0
     * 或拿模型目录的窗口值冒充。这一段只给已用/窗口：更新时刻收在展开的明细卡里，
     * 行尾不再挂「更新于 N 秒前」的尾巴。
     */
    const snapshot = input.contextSnapshot;
    if (snapshot !== null && snapshot.usedTokens !== null && snapshot.usedTokens > 0) {
      segments.push({
        kind: 'context',
        text: contextPairText(snapshot.usedTokens, snapshot.windowTokens),
        tone: contextPressureTone(snapshot.usedTokens, snapshot.windowTokens),
        title: snapshot.title,
        icon: 'ring',
      });
      // 剩余量紧跟在上下文之后：窗口减去实测占用，两者任一未知时不出现。
      if (typeof snapshot.remainingTokens === 'number') {
        segments.push({
          kind: 'remaining',
          text: shortMetric('剩余', formatDecimalCount(snapshot.remainingTokens)),
          title: '距离模型窗口还剩多少 token（窗口减去 Provider 实测占用）',
          tone: 'default',
        });
      }
    }
  }
  // 5 计划：有步骤时才出现。
  if (input.plan !== null && input.plan.total > 0) {
    segments.push({
      kind: 'plan',
      text: shortMetric('计划', `${input.plan.completed}/${input.plan.total}`),
      icon: 'list',
      tone: 'default',
        });
  }
  // 7 首字。
  if (input.firstTokenAvgMs !== null) {
    segments.push({
      kind: 'firstToken',
      text: shortMetric('首字', formatDurationMillisCompact(input.firstTokenAvgMs)),
      icon: 'clock',
      tone: 'default',
        });
  }
  // 8 速率。
  if (input.generationTpsX10 !== null) {
    segments.push({
      kind: 'generationRate',
      text: shortMetric('速率', formatTokensPerSecondX10(input.generationTpsX10)),
      icon: 'gauge',
      tone: 'default',
        });
  }
  // 9 耗时已并入上面的会话累计：这一行不再重复报单轮耗时。
  return segments;
}

/** 指标条的四簇：身份、会话读数、性能、辅助。 */
export type HeaderClusterKind = 'session' | 'performance' | 'aux';

/** 指标条的一段成簇结果。 */
export interface HeaderCluster {
  kind: HeaderClusterKind;
  segments: HeaderSegment[];
}

/** 段到簇的归属；不在表里的段不出现。 */
const CLUSTER_OF: Record<HeaderSegmentKind, HeaderClusterKind> = {
  reasoning: 'session',
  sessionTokens: 'session',
  elapsed: 'session',
  cacheHit: 'session',
  context: 'session',
  remaining: 'session',
  plan: 'aux',
  lsp: 'aux',
  firstToken: 'performance',
  generationRate: 'performance',
};

/**
 * 把平铺的段按语义分成四簇。
 *
 * 四簇的关系是「在不在干活 · 这条会话花了多少 · 跑得多快 · 辅助状态」，
 * 因此装不下时整簇收起，而不是从中间把某一簇切成两半。
 * 工作区路径不进指标条：侧栏与窗口标题已经说清楚当前在哪，顶部再写一遍只是重复。
 * LSP 与计划是辅助读数，固定排在最后，窗口不够时先让位；MCP 台数与技能条数属
 * 「这台机器上挂着什么」，由侧栏底栏的连接行常驻承载，不占指标条。
 */
export function headerClusters(input: HeaderMetricsInput): HeaderCluster[] {
  const groups: HeaderCluster[] = [
    { kind: 'session', segments: [] },
    { kind: 'performance', segments: [] },
    { kind: 'aux', segments: [] },
  ];
  for (const segment of headerSegments(input)) {
    groups.find((group) => group.kind === CLUSTER_OF[segment.kind])?.segments.push(segment);
  }
  return groups.filter((group) => group.segments.length > 0);
}
