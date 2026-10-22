/*
 * 展示层格式化：与 TUI 的 axon-display 保持同一口径。
 *
 * 指标条必须与 TUI 恒等映射，因此这里的每一个数字格式都不是「大致像」，
 * 而是照抄后端规则：千进制单位阶梯、两位小数去尾零、时长分档、
 * 万分比固定两位百分比、按显示宽度截断并追加省略号。
 */

/** 千进制单位阶梯，从大到小。 */
const DECIMAL_UNITS: Array<[number, string]> = [
  [1_000_000_000_000, 'T'],
  [1_000_000_000, 'B'],
  [1_000_000, 'M'],
  [1_000, 'K'],
];

const MILLIS_PER_SECOND = 1_000;
const MILLIS_PER_MINUTE = 60 * MILLIS_PER_SECOND;
const MILLIS_PER_HOUR = 60 * MILLIS_PER_MINUTE;
const MILLIS_PER_DAY = 24 * MILLIS_PER_HOUR;
/** 秒级时长的显示小数位。 */
const SECOND_DISPLAY_DECIMALS = 2;
/** 万分比每百分点。 */
const BASIS_POINTS_PER_PERCENT = 100;

/** 按小数位格式化并去掉尾零。 */
function trimDecimal(value: number, decimals: number): string {
  let formatted = value.toFixed(decimals);
  if (formatted.includes('.')) {
    while (formatted.endsWith('0')) formatted = formatted.slice(0, -1);
    if (formatted.endsWith('.')) formatted = formatted.slice(0, -1);
  }
  return formatted;
}

/** 单位缩放后保留几位小数：小于 10 两位，小于 100 一位，否则零位。 */
function decimalPlaces(scaled: number): number {
  if (scaled < 10) return 2;
  return scaled < 100 ? 1 : 0;
}

/** 把十进制计数压缩为短文本，例如 2300 得到 2.3K。 */
export function formatDecimalCount(value: number): string {
  for (const [unitValue, suffix] of DECIMAL_UNITS) {
    if (value >= unitValue) {
      const scaled = value / unitValue;
      return `${trimDecimal(scaled, decimalPlaces(scaled))}${suffix}`;
    }
  }
  return String(value);
}

/** 秒级时长按 10ms 对齐后再显示。 */
function roundedMillisForSeconds(durationMs: number): number {
  const factor = 10 ** (3 - SECOND_DISPLAY_DECIMALS);
  return Math.round(durationMs / factor) * factor;
}

/** 把毫秒压缩为短文本，例如 8000 得到 8s。 */
export function formatDurationMillisCompact(durationMs: number): string {
  if (durationMs < MILLIS_PER_SECOND) return `${durationMs}ms`;
  if (durationMs < MILLIS_PER_MINUTE) {
    const rounded = roundedMillisForSeconds(durationMs);
    if (rounded >= MILLIS_PER_MINUTE) return formatDurationMillisCompact(rounded);
    return `${trimDecimal(rounded / MILLIS_PER_SECOND, SECOND_DISPLAY_DECIMALS)}s`;
  }
  if (durationMs < MILLIS_PER_HOUR) {
    const minutes = Math.floor(durationMs / MILLIS_PER_MINUTE);
    const seconds = Math.floor((durationMs % MILLIS_PER_MINUTE) / MILLIS_PER_SECOND);
    return `${minutes}m ${String(seconds).padStart(2, '0')}s`;
  }
  if (durationMs < MILLIS_PER_DAY) {
    const hours = Math.floor(durationMs / MILLIS_PER_HOUR);
    const minutes = Math.floor((durationMs % MILLIS_PER_HOUR) / MILLIS_PER_MINUTE);
    const seconds = Math.floor((durationMs % MILLIS_PER_MINUTE) / MILLIS_PER_SECOND);
    return `${hours}h ${String(minutes).padStart(2, '0')}m ${String(seconds).padStart(2, '0')}s`;
  }
  const days = Math.floor(durationMs / MILLIS_PER_DAY);
  const hours = Math.floor((durationMs % MILLIS_PER_DAY) / MILLIS_PER_HOUR);
  const minutes = Math.floor((durationMs % MILLIS_PER_HOUR) / MILLIS_PER_MINUTE);
  return `${days}d ${String(hours).padStart(2, '0')}h ${String(minutes).padStart(2, '0')}m`;
}

/** 时长的进位拆分；天是最大档，再长的任务也只说到天。 */
function durationParts(durationMs: number): {
  seconds: number;
  minutes: number;
  hours: number;
  days: number;
  totalMinutes: number;
  totalHours: number;
} {
  const totalSeconds = Math.floor(Math.max(0, durationMs) / MILLIS_PER_SECOND);
  const totalMinutes = Math.floor(totalSeconds / 60);
  const totalHours = Math.floor(totalMinutes / 60);
  return {
    seconds: totalSeconds % 60,
    minutes: totalMinutes % 60,
    hours: totalHours % 24,
    days: Math.floor(totalHours / 24),
    totalMinutes,
    totalHours,
  };
}

/**
 * 带单位的时长文案，例如 2分6秒。
 *
 * 后台任务行的口径：量级越大精度越低，秒不补零，小时档只说到分。
 * 与参考实现的作业列表逐值一致，因此同一屏里的耗时读起来是同一套。
 */
export function formatDurationUnits(durationMs: number): string {
  const { seconds, minutes, hours, days, totalMinutes, totalHours } = durationParts(durationMs);
  if (days > 0) return hours === 0 ? `${days}天` : `${days}天${hours}小时`;
  if (totalHours > 0) return `${totalHours}小时${minutes}分`;
  if (totalMinutes > 0) return `${totalMinutes}分${seconds}秒`;
  return `${seconds}秒`;
}

/**
 * 带单位的时长文案，例如 2分06秒。
 *
 * 子代理行的口径：秒与分补零到两位，因此同一列数字右对齐后位数不跳。
 * 与参考实现的子代理列表逐值一致（它另有不补零的作业行口径）。
 */
export function formatDurationPadded(durationMs: number): string {
  const { seconds, minutes, hours, days, totalMinutes, totalHours } = durationParts(durationMs);
  const paddedSeconds = String(seconds).padStart(2, '0');
  if (days > 0) return hours === 0 ? `${days}天` : `${days}天${hours}小时`;
  if (totalHours > 0) {
    return `${totalHours}小时${String(minutes).padStart(2, '0')}分${paddedSeconds}秒`;
  }
  if (totalMinutes > 0) return `${totalMinutes}分${paddedSeconds}秒`;
  return `${seconds}秒`;
}

/** 把万分比格式化为固定两位小数的百分比，例如 6060 得到 60.60%。 */
export function formatBasisPointsPercent(value: number): string {
  const whole = Math.floor(value / BASIS_POINTS_PER_PERCENT);
  const fraction = value % BASIS_POINTS_PER_PERCENT;
  return `${whole}.${String(fraction).padStart(2, '0')}%`;
}

/** 横排时长，例如 1h 02m 03s 之外的短格式，用于指标条耗时段。 */
export function formatElapsedCompact(durationMs: number): string {
  return formatDurationMillisCompact(durationMs);
}

/**
 * 估算一个码点的显示宽度。
 *
 * TUI 用 unicode-width 的东亚宽字符表；这里按同一判据近似：CJK、谚文、
 * 全角形式与常见宽符号占两列，其余占一列。
 */
function codePointWidth(codePoint: number): number {
  const wide = [
    [0x1100, 0x115f],
    [0x2e80, 0x303e],
    [0x3041, 0x33ff],
    [0x3400, 0x4dbf],
    [0x4e00, 0x9fff],
    [0xa000, 0xa4cf],
    [0xac00, 0xd7a3],
    [0xf900, 0xfaff],
    [0xfe30, 0xfe6f],
    [0xff00, 0xff60],
    [0xffe0, 0xffe6],
    [0x20000, 0x3fffd],
  ];
  return wide.some(([start, end]) => codePoint >= start && codePoint <= end) ? 2 : 1;
}

/** 计算一段文本的显示宽度。 */
export function displayWidth(text: string): number {
  let width = 0;
  for (const character of text) {
    const codePoint = character.codePointAt(0) ?? 0;
    width += codePointWidth(codePoint);
  }
  return width;
}

/**
 * 按显示宽度截断并追加省略号。
 *
 * 传入的预算是**含省略号**的总宽度，与 TUI 的 ellipsize 一致；
 * 文本本身不超宽时原样返回。
 */
export function ellipsize(text: string, maxWidth: number): string {
  const trimmed = text.trim();
  if (displayWidth(trimmed) <= maxWidth) return trimmed;
  const ellipsis = '…';
  const budget = Math.max(0, maxWidth - displayWidth(ellipsis));
  let kept = '';
  let width = 0;
  for (const character of trimmed) {
    const characterWidth = codePointWidth(character.codePointAt(0) ?? 0);
    if (width + characterWidth > budget) break;
    kept += character;
    width += characterWidth;
  }
  return `${kept}${ellipsis}`;
}

/** 取模型名最后一段，与 TUI 的 model_name_only 同口径。 */
export function modelNameOnly(value: string): string {
  const segments = value.split('/');
  const leaf = (segments[segments.length - 1] ?? '').trim();
  return leaf.length > 0 ? leaf : value.trim();
}

/** 速率：值以 0.1 tok/s 为单位，整十数不显示小数位。 */
export function formatTokensPerSecondX10(value: number): string {
  const whole = Math.floor(value / 10);
  const rest = value % 10;
  return rest === 0 ? `${whole} tok/s` : `${whole}.${rest} tok/s`;
}

/**
 * 消息时间标记：按距当前时间的远近换档，历史会话一眼看出是哪天。
 *
 * 档位固定为「刚刚 / 时:分 / 昨天 时:分 / 月日 时:分 / 年月日 时:分」，
 * 判定用本地日历日而不是 24 小时窗口：跨零点后昨天的消息就该显示「昨天」。
 */
export function formatMessageTime(atMs: number, nowMs: number = Date.now()): string {
  const clock = formatClockTime(atMs);
  const elapsed = nowMs - atMs;
  if (elapsed >= 0 && elapsed < JUST_NOW_MS) return JUST_NOW_TEXT;
  const days = calendarDayGap(atMs, nowMs);
  if (days === 0) return clock;
  if (days === 1) return `昨天 ${clock}`;
  const at = new Date(atMs);
  const now = new Date(nowMs);
  if (at.getFullYear() === now.getFullYear()) {
    return `${at.getMonth() + 1}月${at.getDate()}日 ${clock}`;
  }
  return `${at.getFullYear()}年${at.getMonth() + 1}月${at.getDate()}日 ${clock}`;
}

/** 「刚刚」的时间窗。 */
const JUST_NOW_MS = 60_000;
const JUST_NOW_TEXT = '刚刚';

/** 本地日历日的差：今天为 0，昨天为 1。 */
function calendarDayGap(atMs: number, nowMs: number): number {
  const at = new Date(atMs);
  const now = new Date(nowMs);
  const atStart = new Date(at.getFullYear(), at.getMonth(), at.getDate()).getTime();
  const nowStart = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  return Math.round((nowStart - atStart) / MILLIS_PER_DAY);
}

/** 消息时间标记：时:分，用户气泡与助手操作条共用同一种写法。 */
export function formatClockTime(atMs: number): string {
  return new Date(atMs).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}
