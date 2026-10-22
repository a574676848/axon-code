import type { PriceRule, PricingStyle } from '../../types';

/*
 * 定价的读写换算，全部是纯函数。
 *
 * 数据模型里的单价是**微美元 / 百万 Token**，而没有人用微美元思考价格：
 * 界面一律按「美元 / 百万 Token」读写，换算只在表单提交与回填两处发生。
 * 放成纯函数是为了让这两处共用一套口径，也为了让四舍五入有据可查。
 */

/** 倍率的基准：10000 表示 1.0 倍。 */
export const DEFAULT_MULTIPLIER_BASIS_POINTS = 10_000;

/** 单价上限（微美元 / 百万 Token）：100 万美元 / 百万 Token，够任何真实模型。 */
const MAX_PRICE_MICROS = 1_000_000_000_000;

/** 倍率上限（basis points）：10 倍。 */
const MAX_MULTIPLIER_BASIS_POINTS = 100_000;

/**
 * 把微美元单价写成美元文本（不带货币符号）。
 *
 * 最多四位小数并去掉尾随的零：`800000` 写成 `0.8`，`187500` 写成 `0.1875`。
 * 表单回填用这个，因为输入框里多一个 `$` 只会让人删一次。
 */
export function formatPricePerMillion(micros: number): string {
  if (!Number.isFinite(micros) || micros <= 0) return '0';
  const text = (micros / 1_000_000).toFixed(4);
  return text.replace(/\.?0+$/, '');
}

/** 表格单元格用的写法：带货币符号。 */
export function formatPriceCell(micros: number): string {
  if (!Number.isFinite(micros) || micros <= 0) return '$0';
  return `$${formatPricePerMillion(micros)}`;
}

/**
 * 表格单元格的固定精度写法：同一列始终四位小数。
 *
 * 表格里的小数位必须对齐，删尾零会让 `$0.8` 与 `$0.1875` 参差，
 * 右对齐也读不成一列。表单回填仍用去尾零的写法（见 formatPricePerMillion）。
 */
export function formatPriceCellFixed(micros: number): string {
  const value = Number.isFinite(micros) && micros > 0 ? micros / 1_000_000 : 0;
  return `$${value.toFixed(4)}`;
}

/**
 * 解析表单里的单价文本；非法输入返回 null（由调用方给出具体到字段的提示）。
 *
 * 允许 `0.8`、`0`、`1e-3` 这类写法，但拒绝负数与超出上限的值：
 * 负数单价会让成本算出负值，那比「填错一位」更难查。
 */
export function parsePriceInput(text: string): number | null {
  const trimmed = text.trim();
  if (trimmed === '') return 0;
  const value = Number(trimmed);
  if (!Number.isFinite(value) || value < 0) return null;
  const micros = Math.round(value * 1_000_000);
  if (micros > MAX_PRICE_MICROS) return null;
  return micros;
}

/** 解析倍率文本为 basis points；非法输入返回 null。 */
export function parseMultiplier(text: string): number | null {
  const trimmed = text.trim();
  if (trimmed === '') return DEFAULT_MULTIPLIER_BASIS_POINTS;
  const value = Number(trimmed);
  if (!Number.isFinite(value) || value <= 0) return null;
  const basisPoints = Math.round(value * DEFAULT_MULTIPLIER_BASIS_POINTS);
  if (basisPoints <= 0 || basisPoints > MAX_MULTIPLIER_BASIS_POINTS) return null;
  return basisPoints;
}

/** 把 basis points 写成倍率文本。 */
export function formatMultiplier(basisPoints: number): string {
  if (!Number.isFinite(basisPoints) || basisPoints <= 0) return '1';
  const text = (basisPoints / DEFAULT_MULTIPLIER_BASIS_POINTS).toFixed(4);
  return text.replace(/\.?0+$/, '');
}

/**
 * 计费风格的展示名。
 *
 * 表格列头已经写了「计费风格」，单元格里再带一次后缀是重复的；
 * 需要完整说法的表单与提示保留默认的「风格」后缀。
 */
export function pricingStyleLabel(style: string, compact = false): string {
  const name = normalizePricingStyle(style) === 'claude' ? 'Claude' : 'OpenAI';
  return compact ? name : `${name} 风格`;
}

/**
 * 归一化计费风格。
 *
 * 网关按 `snake_case` 序列化，因此读回来的是 `open_ai`；而写入时后端接受
 * `openai` 与 `open_ai` 两种写法。界面统一收成 `openai` / `claude` 两个取值，
 * 否则「读回来的值再写回去」在表单里会选不中任何一项。
 */
export function normalizePricingStyle(style: string): PricingStyle {
  return style.trim().toLowerCase().startsWith('claude') ? 'claude' : 'openai';
}

/** 规则表里的更新时间：`10/21 16:04`。 */
export function formatRuleTime(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '无记录';
  const date = new Date(ms);
  const clock = `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
  return `${date.getMonth() + 1}/${date.getDate()} ${clock}`;
}

/** 按月与天与小时分档的「多久以前」，用于同步状态行。 */
export function formatAgo(ms: number | null | undefined, now: number): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms <= 0) return '从未运行';
  const delta = Math.max(0, now - ms);
  if (delta < 60_000) return '刚刚';
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)} 分钟前`;
  if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)} 小时前`;
  return `${Math.floor(delta / 86_400_000)} 天前`;
}

/**
 * 过滤规则列表：按匹配式做大小写不敏感的包含匹配。
 *
 * 本地过滤而不是走后端：规则总量在几百条量级，一次性读回来再筛比每敲一个字
 * 打一次网关更快，也不会有输入抖动导致的结果乱序。
 */
export function filterRules(rules: PriceRule[], query: string): PriceRule[] {
  const needle = query.trim().toLowerCase();
  if (needle === '') return rules;
  return rules.filter((rule) => rule.modelPattern.toLowerCase().includes(needle));
}
