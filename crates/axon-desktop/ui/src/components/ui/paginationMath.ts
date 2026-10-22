/*
 * 分页的页码序列：纯函数放在 `.ts` 里，组件与单测共用同一份实现。
 *
 * 页数少时全给；页数多时只给两端与当前页附近，中间以省略号占位。
 */

/** 页码序列中的省略号占位。 */
export const PAGE_GAP = 'gap' as const;

/** 计算页码序列。 */
export function pageNumbers(current: number, total: number): (number | typeof PAGE_GAP)[] {
  if (total <= 7) return Array.from({ length: total }, (_, index) => index + 1);
  const wanted = new Set([1, total, current, current - 1, current + 1]);
  const pages = [...wanted].filter((page) => page >= 1 && page <= total).sort((a, b) => a - b);
  const out: (number | typeof PAGE_GAP)[] = [];
  let previous = 0;
  for (const page of pages) {
    if (previous !== 0 && page - previous > 1) out.push(PAGE_GAP);
    out.push(page);
    previous = page;
  }
  return out;
}
