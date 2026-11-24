/*
 * 非对话页共用的读数格式：时刻、时长、百分比。
 *
 * 自动化任务与记忆时间轴说的是同一批「发生的时刻」，两页必须用同一种写法，
 * 否则同一个时刻在两处读起来像两个事件。三条例行纪律：
 * 1. 同年只写月日，跨年才补年份，避免每一行都重复当前年份；
 * 2. 一分钟以内保留一位小数，超过一分钟收成「1分6秒」，数值与单位之间不留空格；
 * 3. 百分比只保留整数，不出现 12.34% 这种读数。
 */

const MILLISECONDS_PER_SECOND = 1_000;
const SECONDS_PER_MINUTE = 60;
const SECONDS_PER_HOUR = SECONDS_PER_MINUTE * 60;

const pad2 = (value: number): string => String(value).padStart(2, "0");

/** 时刻：同年 `10/21 17:14`，跨年 `2026年10月21日 17:14`。 */
export function formatDateTime(value: number, now = Date.now()): string {
  const date = new Date(value);
  const clock = `${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
  const monthDay = `${date.getMonth() + 1}/${date.getDate()}`;
  if (date.getFullYear() === new Date(now).getFullYear()) return `${monthDay} ${clock}`;
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日 ${clock}`;
}

/** 带缺失值的时刻：没有安排时写「未安排」，其余与 formatDateTime 同一口径。 */
export const formatMoment = (value?: number | null, now = Date.now()): string =>
  value == null ? "未安排" : formatDateTime(value, now);

/** 时长：`1分6秒`；不足一分钟保留一位小数，例如 `0.3 秒`。 */
export function formatDuration(value: number): string {
  const elapsed = Math.max(0, value);
  const total = Math.round(elapsed / MILLISECONDS_PER_SECOND);
  if (total < SECONDS_PER_MINUTE) {
    const seconds = Math.round((elapsed / MILLISECONDS_PER_SECOND) * 10) / 10;
    return `${seconds} 秒`;
  }
  const hours = Math.floor(total / SECONDS_PER_HOUR);
  const minutes = Math.floor((total % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE);
  const seconds = total % SECONDS_PER_MINUTE;
  if (hours > 0) {
    return seconds === 0 ? `${hours}小时${minutes}分` : `${hours}小时${minutes}分${seconds}秒`;
  }
  return seconds === 0 ? `${minutes}分` : `${minutes}分${seconds}秒`;
}

/** 万分比转整数百分比；缺值按 0 处理，与既有展示一致。 */
export const formatPercent = (basisPoints?: number | null): string =>
  `${Math.round((basisPoints ?? 0) / 100)}%`;
