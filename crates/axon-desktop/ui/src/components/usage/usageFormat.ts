import type { UsageAuditEvent } from '../../types';

/*
 * 用量看板的格式化口径。
 *
 * 全部是纯函数：数值怎么读、时间怎么写都在这一处定死，
 * 图表组件只负责几何，不各写一份 toFixed 与拼字符串的规矩。
 *
 * 三条例行纪律：
 * 1. 缺观测就说缺观测，不用 0 冒充（成本未定价时由 costLabel 给出文本）；
 * 2. 读数一律走 tabular-nums，位数变化不带着标题抖（在组件里声明类名）；
 * 3. 时间口径按本地日历日判定，不用「24 小时窗口」近似。
 */

/** 千分位的整数计数。 */
export function formatCount(value: number): string {
  if (!Number.isFinite(value)) return '0';
  return Math.round(value).toLocaleString('en-US');
}

const TOKEN_UNITS = ['', 'K', 'M', 'B', 'T'] as const;

/**
 * Token 体量的紧凑写法：`51.17K`、`27.45M`。
 * 不足一千按整数显示，避免出现 `0.98K` 这种读不出量级的写法。
 */
export function formatTokens(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0';
  let scaled = value;
  let unit = 0;
  while (scaled >= 1000 && unit < TOKEN_UNITS.length - 1) {
    scaled /= 1000;
    unit += 1;
  }
  if (unit === 0) return String(Math.round(scaled));
  return `${scaled.toFixed(2)}${TOKEN_UNITS[unit]}`;
}

/** 万分比（basis points）转百分比文本。 */
export function formatPercent(basisPoints: number, digits = 2): string {
  if (!Number.isFinite(basisPoints)) return '0.00%';
  return `${(basisPoints / 100).toFixed(digits)}%`;
}

/** 0 到 1 的比率转百分比文本。 */
export function formatRatio(ratio: number, digits = 2): string {
  if (!Number.isFinite(ratio)) return '0.00%';
  return `${(ratio * 100).toFixed(digits)}%`;
}

/**
 * 速率读数。
 *
 * 大于等于 10 取整，1 到 10 保留两位小数，小于 1 保留**两位有效数字**（最多四位小数）：
 * 个人用量摊到每分钟常是千分之几次，固定两位小数会一律读成 `0.00`，看起来像坏了；
 * 而按原始位数全写出来（`0.033333`）又是没有意义的精度。两位有效数字两头都避开。
 */
export function formatRate(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0';
  if (value >= 10) return String(Math.round(value));
  if (value >= 1) return value.toFixed(2);
  const decimals = Math.min(4, Math.max(1, 1 - Math.floor(Math.log10(value))));
  const text = value
    .toFixed(decimals)
    .replace(/0+$/, '')
    .replace(/\.$/, '');
  return text === '' || text === '0' ? '<0.0001' : text;
}

/** 延迟读数：毫秒、秒、分钟三档，单位跟着量级走。 */
export function formatLatency(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms <= 0) return '无观测';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(2)}s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes}分${String(seconds).padStart(2, '0')}秒`;
}

/** 生成速率：令牌每秒。 */
export function formatThroughput(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value) || value <= 0) return '无观测';
  return `${value.toFixed(1)} tok/s`;
}

/**
 * 费用读数：微美元转美元。
 *
 * 四位小数是这一层的分辨率：单次请求常在 0.01 美元以下，
 * 两位小数会把绝大多数真实读数抹成 `$0.00`。
 */
export function formatCostMicros(micros: number): string {
  if (!Number.isFinite(micros) || micros <= 0) return '$0.00';
  return `$${(micros / 1_000_000).toFixed(4)}`;
}

/**
 * 成本口径说明。
 *
 * Axon 不预设模型价格，因此「有请求但一分钱都没定价」是常态而不是异常：
 * 这时如实写「未定价」，不画一条全零的成本曲线冒充统计结果。
 */
export function costLabel(micros: number, requestCount: number): string {
  if (requestCount <= 0) return '无请求';
  if (micros <= 0) return '未定价';
  return formatCostMicros(micros);
}

/** 时钟读数：`15:42`。 */
export function formatClock(ms: number): string {
  const date = new Date(ms);
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}

/** 日期读数：`10/21`。 */
export function formatMonthDay(ms: number): string {
  const date = new Date(ms);
  return `${date.getMonth() + 1}/${date.getDate()}`;
}

/**
 * 时刻读数，两档：同年 `10/21 16:04`，跨年 `2026年10月21日 16:04`。
 *
 * 同一年的日期里年份是噪声，省掉一年让表格一列读得下；
 * 跨年时年份是必要信息，补全中文写法。
 */
export function formatDateTime(ms: number, now: number = Date.now()): string {
  const date = new Date(ms);
  const day = `${date.getMonth() + 1}/${date.getDate()} ${formatClock(ms)}`;
  if (date.getFullYear() === new Date(now).getFullYear()) return day;
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日 ${formatClock(ms)}`;
}

/** 相对时间：用于「数据更新于」。 */
export function formatRelative(ms: number, now: number): string {
  const delta = Math.max(0, now - ms);
  if (delta < 10_000) return '刚刚';
  if (delta < 60_000) return `${Math.floor(delta / 1000)} 秒前`;
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)} 分钟前`;
  return `${Math.floor(delta / 3_600_000)} 小时前`;
}

/** 请求来源的展示名。 */
export function sourceLabel(source: string | null | undefined): string {
  switch (source) {
    case 'axon_cli':
      return '主 Agent';
    case 'subagent':
      return '子代理';
    case 'all':
    case undefined:
    case null:
      return '全部来源';
    default:
      return source;
  }
}

/** 请求终态的展示名。 */
export function statusLabel(status: string): string {
  switch (status) {
    case 'completed':
      return '成功';
    case 'failed':
      return '失败';
    case 'interrupted':
      return '中断';
    case 'active':
      return '进行中';
    default:
      return status;
  }
}

/** 终态对应的状态语义，与全站状态色一致。 */
export function statusTone(status: string): 'ok' | 'danger' | 'warning' | 'idle' {
  switch (status) {
    case 'completed':
      return 'ok';
    case 'failed':
      return 'danger';
    case 'interrupted':
      return 'warning';
    default:
      return 'idle';
  }
}

/** 单次请求的 Token 构成，用于明细行的紧凑读数。 */
export function eventTokens(event: UsageAuditEvent): {
  input: number;
  output: number;
  reasoning: number;
  cacheRead: number;
} {
  return {
    input: event.inputTokens,
    output: event.outputTokens,
    reasoning: event.reasoningTokens,
    cacheRead: event.cacheReadTokens,
  };
}
