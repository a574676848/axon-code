/*
 * 用量统计的传输模型。
 *
 * 形状与网关 `/api/v1/observability/*` 的响应逐字段一致（camelCase）。
 * 这些字段是后端事实，界面不在这里补默认值：缺失就是缺失，由展示层显式说明。
 *
 * 与 `types.ts` 分开放：传输模型按领域成篇，单文件才不会越过行数硬限。
 */


/** 统计周期，与网关 period 白名单一致。 */
export type UsagePeriod = 'today' | '7d' | '30d' | 'all';

/** 用量分布维度。 */
export type UsageDimension = 'model' | 'provider';

/** 实时滑窗窗口。 */
export type UsageRealtimeWindow = '15m' | '30m' | '60m';

/** 大盘汇总。 */
export interface UsageOverviewSummary {
  requestCount: number;
  successCount: number;
  failureCount: number;
  successRateBasisPoints: number;
  totalTokens: number;
  totalInputTokens: number;
  cacheReadInputTokens: number;
  cacheWriteInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  cacheHitRateBasisPoints: number;
  currentRpm: number;
  currentTpm: number;
  totalCostMicros: number;
  costFormatted: string;
}

/** 迷你走势序列，每项一个时间桶。 */
export interface UsageSparklines {
  requests: number[];
  tokens: number[];
  costs: number[];
}

/** 活动热力图分块。 */
export interface UsageHeatmapBucket {
  /** 本地日期 `YYYY-MM-DD`。 */
  date: string;
  /** 活跃度等级 0 至 4。 */
  level: number;
  value: number;
}

export interface UsageHeatmap {
  metric: string;
  buckets: UsageHeatmapBucket[];
}

/** 请求健康度档位。 */
export type UsageHealthStatus = 'healthy' | 'degraded' | 'failed';

export interface UsageHealthBucket {
  timestampMs: number;
  status: UsageHealthStatus;
  /** 成功率 0 至 1。 */
  successRate: number;
}

/** 大盘概览响应。 */
export interface UsageOverviewReport {
  summary: UsageOverviewSummary;
  sparklines: UsageSparklines;
  activityHeatmap: UsageHeatmap;
  healthTimeline: { buckets: UsageHealthBucket[] };
}

/** 用量分布分项。 */
export interface UsageDistributionItem {
  key: string;
  tokens: number;
  /** 占全周期 Token 的百分比。 */
  percentage: number;
  requestCount: number;
  costMicros: number;
  cacheHitRateBasisPoints: number;
  tpm: number;
  rpm: number;
}

/** 分布时间序列点，一个时间桶一项。 */
export interface UsageTimeSeriesPoint {
  timestampMs: number;
  uncachedInputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  requestCount: number;
  costMicros: number;
}

/** 用量分布响应。 */
export interface UsageDistributionReport {
  totalTokens: number;
  totalCostMicros: number;
  items: UsageDistributionItem[];
  timeSeries: UsageTimeSeriesPoint[];
}

/** 延迟分位数。 */
export interface UsagePercentiles {
  p50TtftMs: number | null;
  p90TtftMs: number | null;
  p95TtftMs: number | null;
  p99TtftMs: number | null;
  p50LatencyMs: number | null;
  p90LatencyMs: number | null;
  p95LatencyMs: number | null;
  p99LatencyMs: number | null;
}

/** 诊断散点：一次请求的首字延迟、总耗时与体量。 */
export interface UsageScatterPoint {
  ttftMs: number;
  totalDurationMs: number;
  tokens: number;
}

/** 模型单价效益。 */
export interface UsageModelEfficiency {
  model: string;
  costPer1mTokens: number;
}

/** 抽稀披露。 */
export interface UsageSamplingDisclosure {
  sampledOf: number;
  sampleCount: number;
  samplingRatio: number;
}

/** 延迟与效率诊断响应。 */
export interface UsageDiagnosticsReport {
  sampleCount: number;
  percentiles: UsagePercentiles;
  scatterPoints: UsageScatterPoint[];
  modelEfficiency: UsageModelEfficiency[];
  sampling?: UsageSamplingDisclosure;
}

/** 实时滑窗的每分钟吞吐。 */
export interface UsageThroughputPoint {
  timestampMs: number;
  tokenPerMin: number;
  requestPerMin: number;
}

/** 实时滑窗的每分钟请求结果。 */
export interface UsageStatusPoint {
  timestampMs: number;
  successCount: number;
  failureCount: number;
  failureRate: number;
}

/** 滑窗内的 Token 构成。 */
export interface UsageTokenComposition {
  uncachedInputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  outputTokens: number;
  reasoningTokens: number;
}

/** 实时滑窗响应。 */
export interface UsageRealtimeReport {
  window: string;
  windowRequestCount: number;
  windowTotalTokens: number;
  windowCacheHitRateBasisPoints: number;
  windowCostMicros: number;
  metrics: {
    currentTpm: number;
    currentRpm: number;
    averageTpm: number;
    averageRpm: number;
  };
  throughputSeries: UsageThroughputPoint[];
  requestStatusSeries: UsageStatusPoint[];
  tokenComposition: UsageTokenComposition;
  ttftCurrentMs: number;
  ttftAverageMs: number;
  latencyCurrentMs: number;
  latencyAverageMs: number;
}

/** 一条请求事件。 */
export interface UsageAuditEvent {
  recordedAtMs: number;
  model: string;
  upstreamModel?: string | null;
  source: string;
  reasoningEffort?: string | null;
  speedMode?: string | null;
  status: string;
  requestPath?: string | null;
  totalDurationMs: number;
  ttftMs?: number | null;
  generationSpeedTps?: number | null;
  inputTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  cacheHitRateBasisPoints: number;
  pricingStyle: string;
  costMicros?: number | null;
}

/** 请求事件日志响应。 */
export interface UsageAuditLogsReport {
  totalCount: number;
  nextCursor?: string | null;
  hasMore: boolean;
  events: UsageAuditEvent[];
}

/** 事件日志的过滤条件。 */
export interface UsageEventFilter {
  model?: string;
  source?: string;
  status?: string;
}

// ------------------------------------------------------------ 模型定价

/** 计费风格：openai 不单独计价缓存写入，claude 支持。 */
export type PricingStyle = 'openai' | 'claude';

/** 一条模型价格规则；单价单位是微美元每百万 Token。 */
export interface PriceRule {
  id: string;
  /** 模型匹配模式，支持 `*` 与 `?`。 */
  modelPattern: string;
  /** 网关回传的计费风格：`open_ai` 或 `claude`，读入后由界面归一化为 openai / claude。 */
  pricingStyle: string;
  inputPricePerMMicros: number;
  outputPricePerMMicros: number;
  cacheReadPricePerMMicros: number;
  cacheWritePricePerMMicros: number;
  /** 费率倍率，10000 表示 1.0 倍。 */
  multiplierBasisPoints: number;
  /** 优先级，数值越大越优先。 */
  priority: number;
  createdAtMs: number;
  updatedAtMs: number;
}

/** 新建或替换规则的请求体。 */
export interface PriceRuleInput {
  modelPattern: string;
  pricingStyle: PricingStyle;
  inputPricePerMMicros: number;
  outputPricePerMMicros: number;
  cacheReadPricePerMMicros: number;
  cacheWritePricePerMMicros: number;
  multiplierBasisPoints: number;
  priority: number;
}

/** 新建回执。 */
export interface SavePriceRuleResult {
  id: string;
  status: string;
}

/** models.dev 同步回执。 */
export interface SyncPricingResult {
  syncedRulesCount: number;
  source: string;
  updatedAtMs: number;
  /** success / debounced / rate_limited / fallback。 */
  outcome: string;
  /** 结论的补充说明：防抖剩余时间或上游失败原因。 */
  detail?: string | null;
}

/** 历史重算回执。 */
export interface RecalculatePricingResult {
  recalculatedCount: number;
  affectedRollupBuckets: number;
  elapsedMs: number;
}

/** 每日自动同步的任务状态。 */
export interface PricingAutoSyncReport {
  enabled: boolean;
  jobId: string;
  jobExists: boolean;
  /** active / paused / completed / removed / missing。 */
  status: string;
  schedule: string;
  lastRunAtMs?: number | null;
  nextRunAtMs?: number | null;
  lastRunSuccess?: boolean | null;
  lastRunSummary?: string | null;
}
