import type {
  UsageHealthBucket,
  UsageHealthStatus,
  UsageHeatmapBucket,
  UsagePercentiles,
  UsagePeriod,
  UsageScatterPoint,
  UsageTimeSeriesPoint,
} from '../../types';

/*
 * 用量看板的时间分桶与几何派生，全部是纯函数。
 *
 * 网关返回的是**有数据的桶**：空闲时段根本不会出现在序列里。直接按下标平铺会把
 * 三个小时的用量摊满整整一周，读起来像「一直在跑」。因此这里一律先按选定粒度
 * 生成完整时间轴（缺的桶补零），再往轴里灌数据：空档就是空档，看得见。
 *
 * 时间轴按本地日历对齐（小时、天、周都落在用户所在时区的自然边界上），
 * 只有六小时桶用本地整点取整，避免跨时区时桶边与本地日界错位。
 */

/** 趋势图的时间粒度。 */
export type TrendGranularity = 'hour' | 'sixHour' | 'day' | 'week';

/** 单个时间桶的合并读数。 */
export interface TokenBucket {
  startMs: number;
  uncachedInput: number;
  cacheRead: number;
  cacheWrite: number;
  output: number;
  reasoning: number;
  requests: number;
  costMicros: number;
  totalTokens: number;
}

/** Token 五类构成：顺序即堆叠顺序，颜色的语义见 tokens.css 的图表色板。 */
export interface TokenClassSpec {
  key: 'uncachedInput' | 'cacheRead' | 'cacheWrite' | 'output' | 'reasoning';
  label: string;
  color: string;
}

export const TOKEN_CLASSES: readonly TokenClassSpec[] = [
  { key: 'uncachedInput', label: '未缓存输入', color: 'var(--color-chart-input)' },
  { key: 'cacheRead', label: '缓存命中', color: 'var(--color-chart-cache-read)' },
  { key: 'cacheWrite', label: '缓存写入', color: 'var(--color-chart-cache-write)' },
  { key: 'output', label: '生成输出', color: 'var(--color-chart-output)' },
  { key: 'reasoning', label: '推理输出', color: 'var(--color-chart-reasoning)' },
];

/** 时间轴桶数上限：再多柱子就糊成一片，超过即降一档粒度。 */
const MAX_BUCKETS = 180;

const HOUR_MS = 3_600_000;
const DAY_MS = 86_400_000;

/** 本粒度下包含 `ms` 的桶起点。 */
export function bucketStart(ms: number, granularity: TrendGranularity): number {
  const date = new Date(ms);
  switch (granularity) {
    case 'hour':
      date.setMinutes(0, 0, 0);
      return date.getTime();
    case 'sixHour':
      date.setMinutes(0, 0, 0);
      date.setHours(Math.floor(date.getHours() / 6) * 6);
      return date.getTime();
    case 'day':
      date.setHours(0, 0, 0, 0);
      return date.getTime();
    case 'week': {
      date.setHours(0, 0, 0, 0);
      // 周一为一周之始，与文档里的 ISO 星期口径一致。
      const weekday = (date.getDay() + 6) % 7;
      date.setDate(date.getDate() - weekday);
      return date.getTime();
    }
  }
}

/** 下一个桶的起点；用日历加法，跨夏令时不会错位一小时。 */
export function nextBucketStart(ms: number, granularity: TrendGranularity): number {
  const date = new Date(ms);
  switch (granularity) {
    case 'hour':
      date.setHours(date.getHours() + 1);
      return date.getTime();
    case 'sixHour':
      date.setHours(date.getHours() + 6);
      return date.getTime();
    case 'day':
      date.setDate(date.getDate() + 1);
      return date.getTime();
    case 'week':
      date.setDate(date.getDate() + 7);
      return date.getTime();
  }
}

/** 周期对应的默认粒度。 */
export function granularityForPeriod(period: UsagePeriod): TrendGranularity {
  switch (period) {
    case 'today':
      return 'hour';
    case '7d':
      return 'sixHour';
    case '30d':
      return 'day';
    case 'all':
      return 'day';
  }
}

/**
 * 周期对应的时间窗起点。
 *
 * 与网关的 period 口径一致：今天从本地零点起算，其余按天数回溯，
 * 「全部」由调用方给出数据自身的起点。
 */
export function periodStartMs(period: UsagePeriod, now: number): number {
  switch (period) {
    case 'today':
      return bucketStart(now, 'day');
    case '7d':
      return bucketStart(now - 7 * DAY_MS, 'day');
    case '30d':
      return bucketStart(now - 30 * DAY_MS, 'day');
    case 'all':
      return 0;
  }
}

function emptyBucket(startMs: number): TokenBucket {
  return {
    startMs,
    uncachedInput: 0,
    cacheRead: 0,
    cacheWrite: 0,
    output: 0,
    reasoning: 0,
    requests: 0,
    costMicros: 0,
    totalTokens: 0,
  };
}

function accumulate(target: TokenBucket, point: UsageTimeSeriesPoint): void {
  target.uncachedInput += point.uncachedInputTokens;
  target.cacheRead += point.cacheReadTokens;
  target.cacheWrite += point.cacheWriteTokens;
  target.output += point.outputTokens;
  target.reasoning += point.reasoningTokens;
  target.requests += point.requestCount;
  target.costMicros += point.costMicros;
  target.totalTokens =
    target.uncachedInput +
    target.cacheRead +
    target.cacheWrite +
    target.output +
    target.reasoning;
}

/**
 * 把分布时间序列摊进完整时间轴。
 *
 * 返回的数组一定覆盖 `fromMs` 到 `toMs` 的每一个桶（补零），因此柱子的位置
 * 与时间成正比，而不是与「第几条数据」成正比。桶数超过上限时自动降一档粒度。
 */
export function trendBuckets(
  points: UsageTimeSeriesPoint[],
  granularity: TrendGranularity,
  fromMs: number,
  toMs: number,
): { granularity: TrendGranularity; buckets: TokenBucket[] } {
  let current = granularity;
  let buckets = fillBuckets(points, current, fromMs, toMs);
  while (buckets.length > MAX_BUCKETS && current !== 'week') {
    current = coarsen(current);
    buckets = fillBuckets(points, current, fromMs, toMs);
  }
  return { granularity: current, buckets };
}

function coarsen(granularity: TrendGranularity): TrendGranularity {
  switch (granularity) {
    case 'hour':
      return 'sixHour';
    case 'sixHour':
      return 'day';
    case 'day':
      return 'week';
    case 'week':
      return 'week';
  }
}

function fillBuckets(
  points: UsageTimeSeriesPoint[],
  granularity: TrendGranularity,
  fromMs: number,
  toMs: number,
): TokenBucket[] {
  const first = bucketStart(fromMs, granularity);
  const last = bucketStart(toMs, granularity);
  const buckets: TokenBucket[] = [];
  let cursor = first;
  // 上限即护栏：起点异常（例如 0）时不会退化成无限循环。
  while (cursor <= last && buckets.length < 4000) {
    buckets.push(emptyBucket(cursor));
    cursor = nextBucketStart(cursor, granularity);
  }
  if (buckets.length === 0) return buckets;

  const index = new Map<number, TokenBucket>();
  for (const bucket of buckets) index.set(bucket.startMs, bucket);
  for (const point of points) {
    const key = bucketStart(point.timestampMs, granularity);
    const target = index.get(key);
    // 落在窗口之外的点直接丢弃：窗口由周期决定，不由数据决定。
    if (target !== undefined) accumulate(target, point);
  }
  return buckets;
}

/** 时间轴的刻度文案：粒度决定读到哪一位。 */
export function axisLabel(ms: number, granularity: TrendGranularity): string {
  const date = new Date(ms);
  if (granularity === 'hour') {
    return `${String(date.getHours()).padStart(2, '0')}:00`;
  }
  if (granularity === 'sixHour') {
    // 六小时桶跨了日界，只写时刻会让人读不出是哪一天。
    return `${date.getMonth() + 1}/${date.getDate()} ${String(date.getHours()).padStart(2, '0')}:00`;
  }
  return `${date.getMonth() + 1}/${date.getDate()}`;
}

/** 一段时间的完整读数文案，用于悬浮读数与无障碍名称。 */
export function bucketRangeLabel(ms: number, granularity: TrendGranularity): string {
  const start = new Date(ms);
  const end = new Date(nextBucketStart(ms, granularity));
  const clock = (date: Date) =>
    `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
  if (granularity === 'hour') return `${clock(start)} 至 ${clock(end)}`;
  if (granularity === 'sixHour') return `${clock(start)} 至 ${clock(end)}`;
  return `${start.getMonth() + 1}月${start.getDate()}日`;
}

/** 坐标轴分度：0 起算，取整到好读的数（1、2、5 的十倍数）。 */
export function niceTicks(max: number, target = 4): number[] {
  if (!Number.isFinite(max) || max <= 0) return [0];
  const rough = max / Math.max(1, target);
  const magnitude = 10 ** Math.floor(Math.log10(rough));
  const normalized = rough / magnitude;
  const step =
    (normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10) * magnitude;
  const ticks: number[] = [];
  for (let value = 0; value <= max + step * 0.001; value += step) {
    ticks.push(value);
  }
  if (ticks[ticks.length - 1] < max) ticks.push(ticks[ticks.length - 1] + step);
  return ticks;
}

/** 堆叠柱的一段：`from` 与 `to` 是累计值，组件按 y 轴比例映射。 */
export interface StackSegment {
  key: TokenClassSpec['key'];
  label: string;
  color: string;
  value: number;
  from: number;
  to: number;
}

/** 把每个时间桶摊成堆叠段；末段一定落在柱顶，因此柱高恒等于总量。 */
export function stackColumns(
  buckets: TokenBucket[],
  classes: readonly TokenClassSpec[] = TOKEN_CLASSES,
): StackSegment[][] {
  return buckets.map((bucket) => {
    let cursor = 0;
    return classes.map((spec) => {
      const value = bucket[spec.key];
      const segment: StackSegment = {
        key: spec.key,
        label: spec.label,
        color: spec.color,
        value,
        from: cursor,
        to: cursor + value,
      };
      cursor += value;
      return segment;
    });
  });
}

/** 各序列在本周期内的合计，用于图例读数。 */
export function classTotals(
  buckets: TokenBucket[],
  classes: readonly TokenClassSpec[] = TOKEN_CLASSES,
): { spec: TokenClassSpec; total: number }[] {
  return classes.map((spec) => ({
    spec,
    total: buckets.reduce((sum, bucket) => sum + bucket[spec.key], 0),
  }));
}

// ------------------------------------------------------------------ 热力图

/** 日历热力图的一格。 */
export interface CalendarCell {
  /** 本地日期 `YYYY-MM-DD`。 */
  date: string;
  ms: number;
  level: number;
  value: number;
  /** 尚未到来的日期：画成空格，不参与「较少 / 较多」的分级。 */
  future: boolean;
}

function localDateKey(ms: number): string {
  const date = new Date(ms);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(
    date.getDate(),
  ).padStart(2, '0')}`;
}

/**
 * 构造按周分列的日历格子（列 = 周，行 = 周日到周六）。
 *
 * 缺失的日期补 level 0，未来的日期标成 future：热力图读的是「哪天在跑」，
 * 没有数据的日子必须看得出来，不能整周消失。
 */
export function calendarWeeks(
  buckets: UsageHeatmapBucket[],
  weeks: number,
  now: number,
): CalendarCell[][] {
  const valueByDate = new Map(buckets.map((bucket) => [bucket.date, bucket]));
  const todayStart = bucketStart(now, 'day');
  const todayWeekday = new Date(todayStart).getDay();
  const lastColumnStart = todayStart - todayWeekday * DAY_MS;
  const firstColumnStart = lastColumnStart - (weeks - 1) * 7 * DAY_MS;

  const columns: CalendarCell[][] = [];
  for (let column = 0; column < weeks; column += 1) {
    const columnStart = firstColumnStart + column * 7 * DAY_MS;
    const cells: CalendarCell[] = [];
    for (let row = 0; row < 7; row += 1) {
      const ms = bucketStart(columnStart + row * DAY_MS, 'day');
      const date = localDateKey(ms);
      const hit = valueByDate.get(date);
      cells.push({
        date,
        ms,
        level: hit?.level ?? 0,
        value: hit?.value ?? 0,
        future: ms > todayStart,
      });
    }
    columns.push(cells);
  }
  return columns;
}

// ------------------------------------------------------------------ 健康时间线

/** 健康时间线的一格：一个小时的状态，null 表示这一小时没有请求。 */
export interface HealthCell {
  hour: number;
  status: UsageHealthStatus | null;
  successRate: number;
  requests: number;
}

/** 健康时间线的一行：一天 24 格。 */
export interface HealthRow {
  dayMs: number;
  cells: HealthCell[];
}

/** 健康时间线最多显示的天数：再长就退化成一片糊掉的色块。 */
export const HEALTH_MAX_DAYS = 14;

/**
 * 把健康桶摊成「行 = 天、列 = 小时」的网格。
 *
 * 网关只给出有请求的小时，这里同样按完整日历补齐：没有请求的小时是空格而不是绿格，
 * 「没跑」与「跑成功了」必须分得开。
 */
export function healthRows(
  buckets: UsageHealthBucket[],
  fromMs: number,
  toMs: number,
): HealthRow[] {
  const byHour = new Map<number, UsageHealthBucket>();
  for (const bucket of buckets) {
    const key = bucketStart(bucket.timestampMs, 'hour');
    const previous = byHour.get(key);
    // 同一小时出现多行时按「更差的一档」收口：健康度不能因为一行成功就报绿。
    if (previous === undefined || worse(previous.status, bucket.status) === bucket.status) {
      byHour.set(key, bucket);
    }
  }

  const lastDay = bucketStart(toMs, 'day');
  const firstDay = Math.max(bucketStart(fromMs, 'day'), lastDay - (HEALTH_MAX_DAYS - 1) * DAY_MS);
  const rows: HealthRow[] = [];
  for (let dayMs = firstDay; dayMs <= lastDay; dayMs = nextBucketStart(dayMs, 'day')) {
    const cells: HealthCell[] = [];
    for (let hour = 0; hour < 24; hour += 1) {
      const ms = bucketStart(dayMs + hour * HOUR_MS, 'hour');
      const hit = byHour.get(ms);
      cells.push({
        hour,
        status: hit?.status ?? null,
        successRate: hit?.successRate ?? 0,
        requests: hit?.status === undefined ? 0 : 1,
      });
    }
    rows.push({ dayMs, cells });
  }
  return rows;
}

function worse(left: UsageHealthStatus, right: UsageHealthStatus): UsageHealthStatus {
  const rank: Record<UsageHealthStatus, number> = { healthy: 0, degraded: 1, failed: 2 };
  return rank[left] >= rank[right] ? left : right;
}

// ------------------------------------------------------------------ 延迟诊断

/** 分位阶梯的一行：同一分位上的首字延迟与总时长。 */
export interface LadderRow {
  label: string;
  ttftMs: number | null;
  latencyMs: number | null;
}

/** 分位阶梯：四个分位 × 两个指标。 */
export function ladderRows(percentiles: UsagePercentiles): LadderRow[] {
  return [
    { label: 'p50', ttftMs: percentiles.p50TtftMs, latencyMs: percentiles.p50LatencyMs },
    { label: 'p90', ttftMs: percentiles.p90TtftMs, latencyMs: percentiles.p90LatencyMs },
    { label: 'p95', ttftMs: percentiles.p95TtftMs, latencyMs: percentiles.p95LatencyMs },
    { label: 'p99', ttftMs: percentiles.p99TtftMs, latencyMs: percentiles.p99LatencyMs },
  ];
}

/** 散点：归一化坐标（0 至 1，y 轴向上）与按 Token 体量取的点半径。 */
export interface ScatterDot {
  x: number;
  y: number;
  radius: number;
  source: UsageScatterPoint;
}

export interface ScatterGeometry {
  dots: ScatterDot[];
  xMax: number;
  yMax: number;
  /** 过滤掉的空样本数：0 值样本不参与描点，但要在脚注里说清楚。 */
  skipped: number;
}

/**
 * 散点几何：x 为首字延迟、y 为总时长，点面积按 Token 体量开方缩放。
 *
 * 零值样本（失败或未计量的请求）不进散点：它们会在原点堆成一团，
 * 让「延迟分布」读成「大部分请求都很快」，因此单独计数并在脚注披露。
 */
export function scatterGeometry(points: UsageScatterPoint[]): ScatterGeometry {
  const usable = points.filter((point) => point.ttftMs > 0 && point.totalDurationMs > 0);
  const skipped = points.length - usable.length;
  if (usable.length === 0) {
    return { dots: [], xMax: 0, yMax: 0, skipped };
  }
  const xMax = Math.max(...usable.map((point) => point.ttftMs));
  const yMax = Math.max(...usable.map((point) => point.totalDurationMs));
  const tokenMax = Math.max(...usable.map((point) => point.tokens), 1);
  const dots = usable.map((point) => ({
    x: point.ttftMs / xMax,
    y: 1 - point.totalDurationMs / yMax,
    // 面积按体量线性、半径按开方：视觉面积与 Token 成正比，读起来才是直觉。
    radius: 3 + 7 * Math.sqrt(point.tokens / tokenMax),
    source: point,
  }));
  return { dots, xMax, yMax, skipped };
}
